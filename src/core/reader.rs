use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use crossbeam_channel::Sender;

use crate::core::algo::{validate_sectors, BlockState, BlockStats, SECTOR_SIZE};
use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::DirectIoFile;

pub const GIGABYTE: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_READ_CHUNK: usize = 1024 * 1024; // 1MB 读块

#[derive(Debug, Clone)]
pub struct ReadTelemetry {
    pub current_file: u64,
    pub total_files: u64,
    pub total_bytes_read: u64,
    pub current_speed_bps: f64,
    pub eta_seconds: u64,
    pub stats: BlockStats,
    pub is_finished: bool,
    pub error: Option<String>,
    // 扇区热力图采样更新：(扇区全局块索引, 采样健康状态)
    pub heatmap_sample: Option<(usize, BlockState)>,
}

pub struct F3ReaderConfig {
    pub target_dir: PathBuf,
    pub salt: u64,
    pub chunk_size: usize,
}

pub struct F3Reader {
    config: F3ReaderConfig,
    cancel_flag: Arc<AtomicBool>,
}

impl F3Reader {
    pub fn new(config: F3ReaderConfig, cancel_flag: Arc<AtomicBool>) -> Self {
        Self { config, cancel_flag }
    }

    // 发现目标目录下所有以 .h2w 结尾的文件，并按数字顺序排序
    pub fn discover_h2w_files(dir: &Path) -> Vec<(u64, PathBuf)> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name.ends_with(".h2w") {
                        let prefix = &file_name[..file_name.len() - 4];
                        if let Ok(num) = prefix.parse::<u64>() {
                            if num > 0 {
                                files.push((num - 1, path));
                            }
                        }
                    }
                }
            }
        }
        files.sort_by_key(|(num, _)| *num);
        files
    }

    pub fn run(&self, tx: Sender<ReadTelemetry>) {
        let chunk_size = if self.config.chunk_size == 0 {
            DEFAULT_READ_CHUNK
        } else {
            self.config.chunk_size
        };

        let mut buf = match AlignedBuffer::new(chunk_size) {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(ReadTelemetry {
                    current_file: 0,
                    total_files: 0,
                    total_bytes_read: 0,
                    current_speed_bps: 0.0,
                    eta_seconds: 0,
                    stats: BlockStats::default(),
                    is_finished: true,
                    error: Some(e),
                    heatmap_sample: None,
                });
                return;
            }
        };

        let files = Self::discover_h2w_files(&self.config.target_dir);
        let total_files = files.len() as u64;

        if total_files == 0 {
            let _ = tx.send(ReadTelemetry {
                current_file: 0,
                total_files: 0,
                total_bytes_read: 0,
                current_speed_bps: 0.0,
                eta_seconds: 0,
                stats: BlockStats::default(),
                is_finished: true,
                error: Some("未在目标路径下找到任何 .h2w 测试文件".to_string()),
                heatmap_sample: None,
            });
            return;
        }

        // 统计所有待校验文件总大小以计算精准读取进度和 ETA
        let total_expected_bytes: u64 = files.iter().map(|(_, p)| {
            std::fs::metadata(p).map(|m| m.len()).unwrap_or(GIGABYTE)
        }).sum();

        let mut stats = BlockStats::default();
        let mut total_read = 0u64;
        let start_time = Instant::now();
        let mut last_sample_time = Instant::now();
        let mut last_sample_bytes = 0u64;
        let mut heatmap_block_counter = 0usize;

        for (file_idx, (num, file_path)) in files.iter().enumerate() {
            if self.cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            let mut direct_file = match DirectIoFile::open_read(file_path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(ReadTelemetry {
                        current_file: (file_idx + 1) as u64,
                        total_files,
                        total_bytes_read: total_read,
                        current_speed_bps: 0.0,
                        eta_seconds: 0,
                        stats,
                        is_finished: true,
                        error: Some(format!("无法打开文件 {:?}: {}", file_path, e)),
                        heatmap_sample: None,
                    });
                    return;
                }
            };

            let mut file_offset = 0u64;

            loop {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    break;
                }

                match direct_file.read_at(&mut buf, file_offset) {
                    Ok(0) => break, // EOF
                    Ok(bytes_read) => {
                        let aligned_bytes = bytes_read - (bytes_read % SECTOR_SIZE);
                        if aligned_bytes > 0 {
                            let expected_start_offset = (*num * GIGABYTE) + file_offset;
                            let prev_stats = stats;
                            validate_sectors(
                                &buf[..aligned_bytes],
                                expected_start_offset,
                                self.config.salt,
                                &mut stats,
                            );

                            // 采样热力图状态：如果本批次有坏块则标记为坏块，否则标记为良好
                            let dominant_state = if stats.bad > prev_stats.bad {
                                BlockState::Bad
                            } else if stats.overwritten > prev_stats.overwritten {
                                BlockState::Overwritten
                            } else if stats.changed > prev_stats.changed {
                                BlockState::Changed
                            } else {
                                BlockState::Good
                            };

                            // 深度修复：全盘容量百分比线性归一化映射 (0..1000 格)
                            let cell_idx = if total_expected_bytes > 0 {
                                ((total_read as f64 / total_expected_bytes as f64) * 999.0) as usize
                            } else {
                                0
                            };
                            let sample = (cell_idx.min(999), dominant_state);

                            total_read += bytes_read as u64;
                            file_offset += bytes_read as u64;

                            let elapsed = last_sample_time.elapsed().as_secs_f64();
                            if elapsed >= 0.3 {
                                let speed = (total_read - last_sample_bytes) as f64 / elapsed;
                                last_sample_time = Instant::now();
                                last_sample_bytes = total_read;

                                let eta = if speed > 50_000.0 && total_expected_bytes > total_read {
                                    ((total_expected_bytes - total_read) as f64 / speed) as u64
                                } else {
                                    0
                                };

                                let _ = tx.send(ReadTelemetry {
                                    current_file: (file_idx + 1) as u64,
                                    total_files,
                                    total_bytes_read: total_read,
                                    current_speed_bps: speed,
                                    eta_seconds: eta,
                                    stats,
                                    is_finished: false,
                                    error: None,
                                    heatmap_sample: Some(sample),
                                });
                            }
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ReadTelemetry {
                            current_file: (file_idx + 1) as u64,
                            total_files,
                            total_bytes_read: total_read,
                            current_speed_bps: 0.0,
                            eta_seconds: 0,
                            stats,
                            is_finished: true,
                            error: Some(format!("读取错误: {}", e)),
                            heatmap_sample: None,
                        });
                        return;
                    }
                }
            }
        }

        let total_elapsed = start_time.elapsed().as_secs_f64();
        let avg_speed = if total_elapsed > 0.0 {
            total_read as f64 / total_elapsed
        } else {
            0.0
        };

        let _ = tx.send(ReadTelemetry {
            current_file: total_files,
            total_files,
            total_bytes_read: total_read,
            current_speed_bps: avg_speed,
            eta_seconds: 0,
            stats,
            is_finished: true,
            error: None,
            heatmap_sample: None,
        });
    }
}
