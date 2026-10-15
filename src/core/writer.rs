use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use crossbeam_channel::Sender;

use crate::core::algo::fill_sectors;
use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::DirectIoFile;

pub const GIGABYTE: u64 = 1024 * 1024 * 1024; // 1 GiB 单文件规格
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024; // 默认 1MB 块大小

#[derive(Debug, Clone)]
pub struct WriteTelemetry {
    pub current_file_index: u64,
    pub total_bytes_written: u64,
    pub current_speed_bps: f64,
    pub eta_seconds: u64,
    pub is_finished: bool,
    pub error: Option<String>,
}

pub struct F3WriterConfig {
    pub target_dir: PathBuf,
    pub start_file_num: u64,
    pub max_bytes: Option<u64>,
    pub chunk_size: usize,
    pub salt: u64,
}

pub struct F3Writer {
    config: F3WriterConfig,
    cancel_flag: Arc<AtomicBool>,
}

impl F3Writer {
    pub fn new(config: F3WriterConfig, cancel_flag: Arc<AtomicBool>) -> Self {
        Self { config, cancel_flag }
    }

    pub fn run(&self, tx: Sender<WriteTelemetry>) {
        let chunk_size = if self.config.chunk_size == 0 {
            DEFAULT_CHUNK_SIZE
        } else {
            self.config.chunk_size
        };

        let mut buf_a = match AlignedBuffer::new(chunk_size) {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(WriteTelemetry {
                    current_file_index: 0,
                    total_bytes_written: 0,
                    current_speed_bps: 0.0,
                    eta_seconds: 0,
                    is_finished: true,
                    error: Some(e),
                });
                return;
            }
        };

        // 深度复刻 F3：如果未显式指定最大字节数，自动探测目标磁盘真实的空闲可用字节数
        let target_free_bytes = if let Some(max) = self.config.max_bytes {
            max
        } else {
            let mut free_avail = 0u64;
            let mut total_b = 0u64;
            let mut total_free = 0u64;
            let path_str = self.config.target_dir.to_string_lossy();
            let mut wide: Vec<u16> = path_str.encode_utf16().collect();
            wide.push(0);
            unsafe {
                if GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free_avail), Some(&mut total_b), Some(&mut total_free)).is_ok() {
                    free_avail
                } else {
                    0
                }
            }
        };

        let mut file_index = self.config.start_file_num;
        let mut total_written = 0u64;
        let start_time = Instant::now();
        let mut last_sample_time = Instant::now();
        let mut last_sample_bytes = 0u64;

        loop {
            if self.cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            if let Some(max) = self.config.max_bytes {
                if total_written >= max {
                    break;
                }
            }

            let file_name = format!("{}.h2w", file_index + 1);
            let file_path = self.config.target_dir.join(&file_name);

            let mut direct_file = match DirectIoFile::create_write(&file_path) {
                Ok(f) => f,
                Err(e) => {
                    // 若创建失败可能是磁盘已满
                    let _ = tx.send(WriteTelemetry {
                        current_file_index: file_index + 1,
                        total_bytes_written: total_written,
                        current_speed_bps: 0.0,
                        eta_seconds: 0,
                        is_finished: true,
                        error: Some(format!("磁盘空间已满或无法创建文件: {}", e)),
                    });
                    break;
                }
            };

            let mut file_offset = 0u64;
            let mut file_full = false;

            while file_offset < GIGABYTE {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    break;
                }

                let remaining_in_file = GIGABYTE - file_offset;
                let to_write = std::cmp::min(chunk_size as u64, remaining_in_file) as usize;
                let to_write_aligned = (to_write + 511) & !511;

                let global_offset = (file_index * GIGABYTE) + file_offset;
                fill_sectors(&mut buf_a[..to_write_aligned], global_offset, self.config.salt);

                match direct_file.write_at(&buf_a[..to_write_aligned], file_offset) {
                    Ok(written) => {
                        let actual_useful = std::cmp::min(written, to_write);
                        total_written += actual_useful as u64;
                        file_offset += actual_useful as u64;

                        // 遥测速度与采样
                        let elapsed_sample = last_sample_time.elapsed().as_secs_f64();
                        if elapsed_sample >= 0.3 {
                            let speed = (total_written - last_sample_bytes) as f64 / elapsed_sample;
                            last_sample_time = Instant::now();
                            last_sample_bytes = total_written;

                            let eta = if speed > 50_000.0 && target_free_bytes > total_written {
                                ((target_free_bytes - total_written) as f64 / speed) as u64
                            } else {
                                0
                            };

                            let _ = tx.send(WriteTelemetry {
                                current_file_index: file_index + 1,
                                total_bytes_written: total_written,
                                current_speed_bps: speed,
                                eta_seconds: eta,
                                is_finished: false,
                                error: None,
                            });
                        }
                    }
                    Err(_) => {
                        // 写入失败说明当前分区物理空间彻底见底
                        file_full = true;
                        if file_offset > 0 {
                            let _ = direct_file.truncate_to(file_offset);
                        }
                        break;
                    }
                }
            }

            if file_full || self.cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            file_index += 1;
        }

        let total_elapsed = start_time.elapsed().as_secs_f64();
        let avg_speed = if total_elapsed > 0.0 {
            total_written as f64 / total_elapsed
        } else {
            0.0
        };

        let _ = tx.send(WriteTelemetry {
            current_file_index: file_index + 1,
            total_bytes_written: total_written,
            current_speed_bps: avg_speed,
            eta_seconds: 0,
            is_finished: true,
            error: None,
        });
    }
}