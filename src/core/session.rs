use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use crossbeam_channel::Sender;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use crate::core::algo::{fill_sectors, validate_sectors, BlockState, BlockStats, SECTOR_SIZE};
use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::DirectIoFile;

pub const GIGABYTE: u64 = 1024 * 1024 * 1024; // 1 GiB 标准分卷
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024; // 默认 1MB 块大小

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2wMode {
    Write,
    Verify,
    WriteAndVerify, // 一键连续写验
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2wPhase {
    Writing,
    Verifying,
    CleaningUp,
    Completed,
    Aborted,
    Failed,
}

#[derive(Debug, Clone)]
pub struct H2wTelemetry {
    pub mode: H2wMode,
    pub phase: H2wPhase,
    pub current_file: u64,
    pub total_files: Option<u64>,
    pub bytes_processed: u64,
    pub total_bytes_expected: u64,
    pub speed_bps: f64,
    pub eta_seconds: u64,
    pub stats: BlockStats,
    pub heatmap_sample: Option<(usize, BlockState)>,
    pub is_finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct H2wConfig {
    pub target_dir: PathBuf,
    pub mode: H2wMode,
    pub start_file_num: u64,
    pub end_file_num: Option<u64>,
    pub max_bytes: Option<u64>,
    pub chunk_size: usize,
    pub salt: u64,
    pub auto_cleanup: bool,
}

// 统一的平滑速率与 ETA 状态机 (消除双端重复采样计算样板代码)
struct ProgressTracker {
    start_time: Instant,
    last_sample_time: Instant,
    last_sample_bytes: u64,
    target_bytes: u64,
}

impl ProgressTracker {
    fn new(target_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            start_time: now,
            last_sample_time: now,
            last_sample_bytes: 0,
            target_bytes,
        }
    }

    fn sample(&mut self, current_bytes: u64) -> Option<(f64, u64)> {
        let elapsed = self.last_sample_time.elapsed().as_secs_f64();
        if elapsed >= 0.3 {
            let speed = (current_bytes - self.last_sample_bytes) as f64 / elapsed;
            self.last_sample_time = Instant::now();
            self.last_sample_bytes = current_bytes;
            let eta = if speed > 50_000.0 && self.target_bytes > current_bytes {
                ((self.target_bytes - current_bytes) as f64 / speed) as u64
            } else {
                0
            };
            Some((speed, eta))
        } else {
            None
        }
    }

    fn avg_speed(&self, current_bytes: u64) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            current_bytes as f64 / elapsed
        } else {
            0.0
        }
    }
}

// H2W 深度测试会话 (H2wSession)
// 统一管理 Direct I/O、对齐缓冲复用、文件切分轮转、扇区填充比对与自动清理
pub struct H2wSession {
    config: H2wConfig,
    cancel_flag: Arc<AtomicBool>,
}

impl H2wSession {
    pub fn new(config: H2wConfig, cancel_flag: Arc<AtomicBool>) -> Self {
        Self { config, cancel_flag }
    }

    // 发现目标目录下所有以 .h2w 结尾的文件，并按数字编号升序排序
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

    // 清理目标目录下所有的 .h2w 测试文件，返回删除数量与释放的总字节数
    pub fn cleanup_h2w_files(dir: &Path) -> Result<(usize, u64), String> {
        let files = Self::discover_h2w_files(dir);
        let mut deleted_count = 0usize;
        let mut freed_bytes = 0u64;
        for (_, path) in files {
            if let Ok(meta) = std::fs::metadata(&path) {
                freed_bytes += meta.len();
            }
            if std::fs::remove_file(&path).is_ok() {
                deleted_count += 1;
            }
        }
        Ok((deleted_count, freed_bytes))
    }

    // 运行测试会话
    pub fn run(&self, tx: Sender<H2wTelemetry>) {
        let chunk_size = if self.config.chunk_size == 0 {
            DEFAULT_CHUNK_SIZE
        } else {
            self.config.chunk_size
        };

        let mut buf = match AlignedBuffer::new(chunk_size) {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(H2wTelemetry {
                    mode: self.config.mode,
                    phase: H2wPhase::Failed,
                    current_file: 0,
                    total_files: None,
                    bytes_processed: 0,
                    total_bytes_expected: 0,
                    speed_bps: 0.0,
                    eta_seconds: 0,
                    stats: BlockStats::default(),
                    heatmap_sample: None,
                    is_finished: true,
                    error: Some(e),
                });
                return;
            }
        };

        match self.config.mode {
            H2wMode::Write => {
                self.execute_write(&mut buf, &tx);
            }
            H2wMode::Verify => {
                self.execute_verify(&mut buf, &tx);
            }
            H2wMode::WriteAndVerify => {
                let write_ok = self.execute_write(&mut buf, &tx);
                if write_ok && !self.cancel_flag.load(Ordering::Relaxed) {
                    self.execute_verify(&mut buf, &tx);
                }
            }
        }
    }

    // 内部写入执行体 (复用 AlignedBuffer，无多余分配)
    fn execute_write(&self, buf: &mut AlignedBuffer, tx: &Sender<H2wTelemetry>) -> bool {
        let target_free_bytes = if let Some(max) = self.config.max_bytes {
            max
        } else {
            let mut free_avail = 0u64;
            let mut total_b = 0u64;
            let mut total_free = 0u64;
            let path_str = self.config.target_dir.to_string_lossy();
            let mut wide: Vec<u16> = path_str.encode_utf16().collect();
            wide.push(0);
            let ok = unsafe {
                GetDiskFreeSpaceExW(
                    PCWSTR(wide.as_ptr()),
                    Some(&mut free_avail),
                    Some(&mut total_b),
                    Some(&mut total_free),
                )
            };
            if ok.is_ok() {
                free_avail
            } else {
                0
            }
        };

        let mut tracker = ProgressTracker::new(target_free_bytes);
        let mut file_index = self.config.start_file_num;
        let mut total_written = 0u64;

        loop {
            if self.cancel_flag.load(Ordering::Relaxed) {
                let _ = tx.send(self.make_telemetry(
                    H2wPhase::Aborted,
                    file_index + 1,
                    None,
                    total_written,
                    target_free_bytes,
                    tracker.avg_speed(total_written),
                    0,
                    BlockStats::default(),
                    None,
                    true,
                    None,
                ));
                return false;
            }

            if let Some(end) = self.config.end_file_num {
                if file_index > end {
                    break;
                }
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
                    let _ = tx.send(self.make_telemetry(
                        H2wPhase::Failed,
                        file_index + 1,
                        None,
                        total_written,
                        target_free_bytes,
                        0.0,
                        0,
                        BlockStats::default(),
                        None,
                        true,
                        Some(format!("创建文件失败: {}", e)),
                    ));
                    return false;
                }
            };

            let mut file_offset = 0u64;
            let mut file_full = false;

            while file_offset < GIGABYTE {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    break;
                }

                let remaining = GIGABYTE - file_offset;
                let to_write = std::cmp::min(buf.len() as u64, remaining) as usize;
                let to_write_aligned = (to_write + 511) & !511;
                let global_offset = (file_index * GIGABYTE) + file_offset;

                fill_sectors(&mut buf[..to_write_aligned], global_offset, self.config.salt);

                match direct_file.write_at(&buf[..to_write_aligned], file_offset) {
                    Ok(written) => {
                        let actual = std::cmp::min(written, to_write);
                        total_written += actual as u64;
                        file_offset += actual as u64;

                        if let Some((speed, eta)) = tracker.sample(total_written) {
                            let _ = tx.send(self.make_telemetry(
                                H2wPhase::Writing,
                                file_index + 1,
                                None,
                                total_written,
                                target_free_bytes,
                                speed,
                                eta,
                                BlockStats::default(),
                                None,
                                false,
                                None,
                            ));
                        }
                    }
                    Err(_) => {
                        // 磁盘已写满是 f3write 正常的终止条件
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

        let is_terminal = self.config.mode == H2wMode::Write;
        let _ = tx.send(self.make_telemetry(
            if is_terminal { H2wPhase::Completed } else { H2wPhase::Writing },
            file_index + 1,
            None,
            total_written,
            target_free_bytes,
            tracker.avg_speed(total_written),
            0,
            BlockStats::default(),
            None,
            is_terminal,
            None,
        ));

        true
    }

    // 内部校验执行体
    fn execute_verify(&self, buf: &mut AlignedBuffer, tx: &Sender<H2wTelemetry>) -> bool {
        let files = Self::discover_h2w_files(&self.config.target_dir);
        let total_files = files.len() as u64;

        if total_files == 0 {
            let _ = tx.send(self.make_telemetry(
                H2wPhase::Failed,
                0,
                Some(0),
                0,
                0,
                0.0,
                0,
                BlockStats::default(),
                None,
                true,
                Some("目标目录下未找到任何 .h2w 测试文件".into()),
            ));
            return false;
        }

        let total_expected_bytes: u64 = files.iter()
            .map(|(_, p)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(GIGABYTE))
            .sum();

        let mut tracker = ProgressTracker::new(total_expected_bytes);
        let mut stats = BlockStats::default();
        let mut total_read = 0u64;

        for (file_idx, (num, file_path)) in files.iter().enumerate() {
            if self.cancel_flag.load(Ordering::Relaxed) {
                let _ = tx.send(self.make_telemetry(
                    H2wPhase::Aborted,
                    (file_idx + 1) as u64,
                    Some(total_files),
                    total_read,
                    total_expected_bytes,
                    tracker.avg_speed(total_read),
                    0,
                    stats,
                    None,
                    true,
                    None,
                ));
                return false;
            }

            let mut direct_file = match DirectIoFile::open_read(file_path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(self.make_telemetry(
                        H2wPhase::Failed,
                        (file_idx + 1) as u64,
                        Some(total_files),
                        total_read,
                        total_expected_bytes,
                        0.0,
                        0,
                        stats,
                        None,
                        true,
                        Some(e),
                    ));
                    return false;
                }
            };

            let mut file_offset = 0u64;

            loop {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    break;
                }

                match direct_file.read_at(buf, file_offset) {
                    Ok(0) => break, // EOF
                    Ok(bytes_read) => {
                        let aligned = bytes_read - (bytes_read % SECTOR_SIZE);
                        if aligned == 0 {
                            break;
                        }

                        let expected_offset = (*num * GIGABYTE) + file_offset;
                        let prev_stats = stats;
                        validate_sectors(&buf[..aligned], expected_offset, self.config.salt, &mut stats);

                        let dominant = if stats.bad > prev_stats.bad {
                            BlockState::Bad
                        } else if stats.overwritten > prev_stats.overwritten {
                            BlockState::Overwritten
                        } else if stats.changed > prev_stats.changed {
                            BlockState::Changed
                        } else {
                            BlockState::Good
                        };

                        let cell_idx = if total_expected_bytes > 0 {
                            (((total_read as f64 / total_expected_bytes as f64) * 999.0) as usize).min(999)
                        } else {
                            0
                        };

                        total_read += bytes_read as u64;
                        file_offset += bytes_read as u64;

                        if let Some((speed, eta)) = tracker.sample(total_read) {
                            let _ = tx.send(self.make_telemetry(
                                H2wPhase::Verifying,
                                (file_idx + 1) as u64,
                                Some(total_files),
                                total_read,
                                total_expected_bytes,
                                speed,
                                eta,
                                stats,
                                Some((cell_idx, dominant)),
                                false,
                                None,
                            ));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(self.make_telemetry(
                            H2wPhase::Failed,
                            (file_idx + 1) as u64,
                            Some(total_files),
                            total_read,
                            total_expected_bytes,
                            0.0,
                            0,
                            stats,
                            None,
                            true,
                            Some(e),
                        ));
                        return false;
                    }
                }
            }
        }

        // 校验完毕后由后台线程无缝执行自动清理 (避免阻塞 UI 主线程)
        if self.config.auto_cleanup && stats.bad == 0 {
            let _ = tx.send(self.make_telemetry(
                H2wPhase::CleaningUp,
                total_files,
                Some(total_files),
                total_read,
                total_expected_bytes,
                0.0,
                0,
                stats,
                None,
                false,
                None,
            ));
            let _ = Self::cleanup_h2w_files(&self.config.target_dir);
        }

        let _ = tx.send(self.make_telemetry(
            H2wPhase::Completed,
            total_files,
            Some(total_files),
            total_read,
            total_expected_bytes,
            tracker.avg_speed(total_read),
            0,
            stats,
            None,
            true,
            None,
        ));

        true
    }

    #[inline]
    fn make_telemetry(
        &self,
        phase: H2wPhase,
        current_file: u64,
        total_files: Option<u64>,
        bytes_processed: u64,
        total_bytes_expected: u64,
        speed_bps: f64,
        eta_seconds: u64,
        stats: BlockStats,
        heatmap_sample: Option<(usize, BlockState)>,
        is_finished: bool,
        error: Option<String>,
    ) -> H2wTelemetry {
        H2wTelemetry {
            mode: self.config.mode,
            phase,
            current_file,
            total_files,
            bytes_processed,
            total_bytes_expected,
            speed_bps,
            eta_seconds,
            stats,
            heatmap_sample,
            is_finished,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_tracker_calculation() {
        let tracker = ProgressTracker::new(100 * 1024 * 1024);
        assert_eq!(tracker.target_bytes, 100 * 1024 * 1024);
        assert_eq!(tracker.avg_speed(0), 0.0);
    }
}
