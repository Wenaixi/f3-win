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
use crate::core::speed::SpeedPhysiologySnapshot;

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
    pub duration_seconds: f64,
    pub physiology: Option<SpeedPhysiologySnapshot>,
    pub first_error_offset: Option<u64>,
    pub first_error_file: Option<u64>,
    pub first_error_expected: Option<[u8; 16]>,
    pub first_error_actual: Option<[u8; 16]>,
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
    pub start_time: Instant,
    last_sample_time: Instant,
    last_sample_bytes: u64,
    target_bytes: u64,
    pub peak_speed_bps: f64,
    pub valley_speed_bps: f64,
    pub slc_cliff_bytes: Option<u64>,
    pub freeze_count: u32,
    pub total_freeze_secs: f64,
}

impl ProgressTracker {
    fn new(target_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            start_time: now,
            last_sample_time: now,
            last_sample_bytes: 0,
            target_bytes,
            peak_speed_bps: 0.0,
            valley_speed_bps: f64::MAX,
            slc_cliff_bytes: None,
            freeze_count: 0,
            total_freeze_secs: 0.0,
        }
    }

    fn sample(&mut self, current_bytes: u64) -> Option<(f64, u64)> {
        let elapsed = self.last_sample_time.elapsed().as_secs_f64();
        if elapsed >= 0.3 {
            let speed = (current_bytes - self.last_sample_bytes) as f64 / elapsed;
            let total_elapsed = self.start_time.elapsed().as_secs_f64();

            // 统计最高峰值速度
            if speed > self.peak_speed_bps {
                self.peak_speed_bps = speed;
            }

            // 统计最低谷底速度 (跳过启动最初 1.5 秒的冷启动阶段)
            if total_elapsed > 1.5 {
                if speed < self.valley_speed_bps {
                    self.valley_speed_bps = speed;
                }
                // 统计严重卡顿停顿 (瞬时速度低于 1MB/s 视为硬件 GC 死锁或总线挂起)
                if speed < 1_000_000.0 {
                    self.freeze_count += 1;
                    self.total_freeze_secs += elapsed;
                }
            }

            // 捕捉 SLC Cache 耗尽拐点：当峰值已大于 20MB/s 且当前速度跌落超过 60% 持续存在
            if self.slc_cliff_bytes.is_none() && self.peak_speed_bps > 20_000_000.0 && total_elapsed > 3.0 {
                if speed < self.peak_speed_bps * 0.4 {
                    self.slc_cliff_bytes = Some(current_bytes);
                }
            }

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
                    duration_seconds: 0.0,
                    physiology: None,
                    first_error_offset: None,
                    first_error_file: None,
                    first_error_expected: None,
                    first_error_actual: None,
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
                    Err(e) => {
                        // 精准判断是否为磁盘已满: 错误码 112 (ERROR_DISK_FULL) 或 39 (ERROR_HANDLE_DISK_FULL)
                        let is_disk_full = e.contains("112")
                            || e.contains("39")
                            || e.contains("空间不足")
                            || e.contains("disk full");

                        if is_disk_full {
                            file_full = true;
                            if file_offset > 0 {
                                let _ = direct_file.truncate_to(file_offset);
                            }
                            break;
                        } else {
                            // 底层遇到真实物理坏道 (23)、设备断开掉盘 (1117) 或写保护，必须立刻报告失败并中止！
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
                                Some(format!("写入中途发生物理 I/O 故障: {}", e)),
                            ));
                            return false;
                        }
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
        let mut first_error_offset: Option<u64> = None;
        let mut first_error_file: Option<u64> = None;
        let mut first_error_actual: Option<[u8; 16]> = None;
        let mut first_error_expected: Option<[u8; 16]> = None;

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

                        // 首发失配故障物理微观现场取证捕获
                        if first_error_offset.is_none() && (stats.bad > prev_stats.bad || stats.overwritten > prev_stats.overwritten || stats.changed > prev_stats.changed) {
                            first_error_offset = Some(expected_offset);
                            first_error_file = Some(*num);

                            // 提取首个故障扇区前 16 字节的实际读回数据
                            let mut actual_16 = [0u8; 16];
                            if aligned >= 16 {
                                actual_16.copy_from_slice(&buf[..16]);
                            }
                            first_error_actual = Some(actual_16);

                            // 计算该扇区预期的首 16 字节数据
                            let mut expected_buf = [0u8; 512];
                            crate::core::algo::fill_sectors(&mut expected_buf, expected_offset, self.config.salt);
                            let mut expected_16 = [0u8; 16];
                            expected_16.copy_from_slice(&expected_buf[..16]);
                            first_error_expected = Some(expected_16);
                        }

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
        // 防伪维权保护守卫：只有在 100% 全绿正品盘 (未发生任何坏道、未发生任何回绕覆写且未发生位翻转) 时才允许自动清理！
        if self.config.auto_cleanup && stats.bad == 0 && stats.overwritten == 0 && stats.changed == 0 {
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
            duration_seconds: 0.0,
            physiology: None,
            first_error_offset: None,
            first_error_file: None,
            first_error_expected: None,
            first_error_actual: None,
        }
    }
}
