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
use crate::core::speed::{SpeedPhysiologySnapshot, SpeedPhysiologyTracker};

pub const GIGABYTE: u64 = 1024 * 1024 * 1024; // 1 GiB 标准分卷
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024; // 默认 1MB 块大小

/// 精准判断 I/O 错误字符串是否代表磁盘物理已满 (Win32 112 ERROR_DISK_FULL 或 39 ERROR_HANDLE_DISK_FULL)
/// 严格匹配标准 [Win32: 112] 标签或中文/英文已满关键字，坚决杜绝因偏移量或其它错误码 (如 1127/1167) 包含 "112" 子串导致误判
#[inline]
pub fn is_disk_full_error(err: &str) -> bool {
    err.contains("[Win32: 112]")
        || err.contains("[Win32: 39]")
        || err.contains(" 112]")
        || err.contains(" 39]")
        || err.contains("ERROR_DISK_FULL")
        || err.contains("空间不足")
        || err.contains("not enough space")
        || err.contains("disk full")
}

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
    // 嵌入高精度生理测速引擎 (Welford 方差 / P² 在线分位数 / SLC 状态机)
    pub physiology: SpeedPhysiologyTracker,
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
            physiology: SpeedPhysiologyTracker::new(target_bytes),
        }
    }

    #[inline]
    pub fn snapshot(&self, current_bytes: u64) -> SpeedPhysiologySnapshot {
        self.physiology.snapshot(current_bytes)
    }

    fn sample(&mut self, current_bytes: u64) -> Option<(f64, u64)> {
        // 直接委托给高精度生理测速引擎，驱动 Welford 方差 / P² 在线分位数 / 3阶段 SLC 断崖防抖状态机
        let (speed, eta, _snap) = self.physiology.sample(current_bytes)?;
        self.peak_speed_bps = self.physiology.peak_speed_bps;
        self.valley_speed_bps = self.physiology.valley_speed_bps;
        self.slc_cliff_bytes = self.physiology.slc_cliff_info.map(|c| c.cliff_bytes);
        self.last_sample_bytes = current_bytes;
        Some((speed, eta))
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

    // 发现目标目录下所有以 .h2w 结尾的有效测试文件，并按数字编号升序排序
    pub fn discover_h2w_files(dir: &Path) -> Vec<(u64, PathBuf)> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                // 安全防呆 1: 必须是标准物理普通文件，杜绝扫描或误删目录与重解析点
                let is_file = entry.file_type().is_ok_and(|ft| ft.is_file());
                if !is_file {
                    continue;
                }

                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    // 安全防呆 2: 支持 Windows 平台大小写不敏感扩展名匹配 (.h2w / .H2W)
                    let is_h2w_ext = file_name.len() > 4
                        && file_name[file_name.len() - 4..].eq_ignore_ascii_case(".h2w");

                    if is_h2w_ext {
                        let prefix = &file_name[..file_name.len() - 4];
                        // 安全防呆 3: 严格校验为非零开头的纯正整数数字，杜绝 '+', '-', 空格与前导零重复
                        let is_valid_number = !prefix.is_empty()
                            && !prefix.starts_with('0')
                            && prefix.chars().all(|c| c.is_ascii_digit());

                        if is_valid_number {
                            if let Ok(num) = prefix.parse::<u64>() {
                                if num > 0 {
                                    files.push((num - 1, path));
                                }
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
                let _ = tx.send(self.make_telemetry_with_tracker(
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
                    Some("用户已中止写入".into()),
                    &tracker,
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
                Ok(f) => Some(f),
                Err(e) => {
                    let _ = tx.send(self.make_telemetry_with_tracker(
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
                        &tracker,
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

                let writer = direct_file.as_mut().unwrap();
                match writer.write_at(&buf[..to_write_aligned], file_offset) {
                    Ok(written) => {
                        let actual = std::cmp::min(written, to_write);
                        total_written += actual as u64;
                        file_offset += actual as u64;

                        if let Some((speed, eta)) = tracker.sample(total_written) {
                            let _ = tx.send(self.make_telemetry_with_tracker(
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
                                &tracker,
                            ));
                        }
                    }
                    Err(e) => {
                        let is_disk_full = is_disk_full_error(&e);

                        if is_disk_full {
                            // 阶梯式降级探测填充末尾可用扇区 (1MB 块写满后，尝试按 64KB -> 4KB -> 512B 粒度写满磁盘末尾)
                            let sector_size = writer.sector_size();
                            let step_sizes = [64 * 1024, 4 * 1024, sector_size];
                            for &step in &step_sizes {
                                if step < sector_size || step >= buf.len() {
                                    continue;
                                }
                                while file_offset + step as u64 <= GIGABYTE {
                                    if self.cancel_flag.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    let step_global_offset = (file_index * GIGABYTE) + file_offset;
                                    fill_sectors(&mut buf[..step], step_global_offset, self.config.salt);
                                    match writer.write_at(&buf[..step], file_offset) {
                                        Ok(written) => {
                                            total_written += written as u64;
                                            file_offset += written as u64;
                                        }
                                        Err(err) => {
                                            let full = is_disk_full_error(&err);
                                            if full {
                                                break;
                                            } else {
                                                let _ = tx.send(self.make_telemetry_with_tracker(
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
                                                    Some(format!("尾部降级写入发生物理 I/O 故障: {}", err)),
                                                    &tracker,
                                                ));
                                                return false;
                                            }
                                        }
                                    }
                                }
                            }

                            file_full = true;
                            if file_offset > 0 {
                                // 截断到实际有效写入的扇区对齐长度并硬件穿透冲刷落盘
                                let trunc = writer.truncate_to(file_offset);
                                let flushed = writer.flush();
                                if let Err(msg) = trunc.and(flushed) {
                                    let _ = tx.send(self.make_telemetry_with_tracker(
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
                                        Some(format!("磁盘已满时落盘失败: {}", msg)),
                                        &tracker,
                                    ));
                                    return false;
                                }
                            } else {
                                // 刚创建分卷未写入任何有效扇区即告满，关闭句柄后清除 0 字节幽灵空文件
                                drop(direct_file.take());
                                let _ = std::fs::remove_file(&file_path);
                            }
                            break;
                        } else {
                            // 底层遇到真实物理坏道 (23)、设备断开掉盘 (1117) 或写保护，必须立刻报告失败并中止！
                            let _ = tx.send(self.make_telemetry_with_tracker(
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
                                &tracker,
                            ));
                            return false;
                        }
                    }
                }
            }

            // 正常写满 1GB 分卷后下发硬件级缓存穿透冲刷，校验落盘状态
            if file_offset >= GIGABYTE {
                if let Some(f) = direct_file.as_mut() {
                    if let Err(e) = f.flush() {
                        let _ = tx.send(self.make_telemetry_with_tracker(
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
                            Some(format!("分卷写入完毕后硬件缓存冲刷失败: {}", e)),
                            &tracker,
                        ));
                        return false;
                    }
                }
            }

            // 用户中途主动取消写入：必须截断并冲刷已写分卷，发送 Aborted，严禁滑向 Completed
            if self.cancel_flag.load(Ordering::Relaxed) {
                if let Some(mut f) = direct_file.take() {
                    if file_offset > 0 {
                        let _ = f.truncate_to(file_offset);
                        let _ = f.flush();
                    }
                }
                let _ = tx.send(self.make_telemetry_with_tracker(
                    H2wPhase::Aborted,
                    file_index + 1,
                    None,
                    total_written,
                    target_free_bytes,
                    0.0,
                    0,
                    BlockStats::default(),
                    None,
                    true,
                    Some("用户已中止写入".into()),
                    &tracker,
                ));
                return false;
            }

            if file_full {
                break;
            }

            file_index += 1;
        }

        let is_terminal = self.config.mode == H2wMode::Write;
        let _ = tx.send(self.make_telemetry_with_tracker(
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
            &tracker,
        ));

        true
    }

    // 内部校验执行体
    fn execute_verify(&self, buf: &mut AlignedBuffer, tx: &Sender<H2wTelemetry>) -> bool {
        let all_files = Self::discover_h2w_files(&self.config.target_dir);
        // 支持断点续验与分段范围校验 (严格对齐 start_file_num 与 end_file_num)
        let files: Vec<(u64, std::path::PathBuf)> = all_files
            .into_iter()
            .filter(|(num, _)| {
                *num >= self.config.start_file_num
                    && self.config.end_file_num.is_none_or(|end| *num <= end)
            })
            .collect();
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
                let _ = tx.send(self.make_telemetry_with_tracker(
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
                    Some("用户已中止校验".into()),
                    &tracker,
                ));
                return false;
            }

            let mut direct_file = match DirectIoFile::open_read(file_path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(self.make_telemetry_with_tracker(
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
                        &tracker,
                    ));
                    return false;
                }
            };

            let mut file_offset = 0u64;

            loop {
                if self.cancel_flag.load(Ordering::Relaxed) {
                    let mut t = self.make_telemetry_with_tracker(
                        H2wPhase::Aborted,
                        (file_idx + 1) as u64,
                        Some(total_files),
                        total_read,
                        total_expected_bytes,
                        0.0,
                        0,
                        stats,
                        None,
                        true,
                        Some("用户已中止校验".into()),
                        &tracker,
                    );
                    t.first_error_offset = first_error_offset;
                    t.first_error_file = first_error_file;
                    t.first_error_expected = first_error_expected;
                    t.first_error_actual = first_error_actual;
                    let _ = tx.send(t);
                    return false;
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
                            let mut t = self.make_telemetry_with_tracker(
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
                                &tracker,
                            );
                            t.first_error_offset = first_error_offset;
                            t.first_error_file = first_error_file;
                            t.first_error_expected = first_error_expected;
                            t.first_error_actual = first_error_actual;
                            let _ = tx.send(t);
                        }
                    }
                    Err(e) => {
                        let mut t = self.make_telemetry_with_tracker(
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
                            &tracker,
                        );
                        t.first_error_offset = first_error_offset;
                        t.first_error_file = first_error_file;
                        t.first_error_expected = first_error_expected;
                        t.first_error_actual = first_error_actual;
                        let _ = tx.send(t);
                        return false;
                    }
                }
            }
        }

        // 若任务已被取消，严禁触发自动清理和假完成
        if self.cancel_flag.load(Ordering::Relaxed) {
            return false;
        }

        // 校验是否完整读取了全部预期的分卷数据，防止残缺或异常空卷被当作 100% 通过
        if total_read < total_expected_bytes {
            let mut t = self.make_telemetry_with_tracker(
                H2wPhase::Failed,
                total_files,
                Some(total_files),
                total_read,
                total_expected_bytes,
                0.0,
                0,
                stats,
                None,
                true,
                Some(format!("校验未完整读取所有扇区: 预期 {} 字节, 实际读取 {} 字节", total_expected_bytes, total_read)),
                &tracker,
            );
            t.first_error_offset = first_error_offset;
            t.first_error_file = first_error_file;
            t.first_error_expected = first_error_expected;
            t.first_error_actual = first_error_actual;
            let _ = tx.send(t);
            return false;
        }

        // 校验完毕后由后台线程无缝执行自动清理 (避免阻塞 UI 主线程)
        // 防伪维权保护守卫：只有在 100% 全绿正品盘 (未发生任何坏道、未发生任何回绕覆写且未发生位翻转) 时才允许自动清理！
        if self.config.auto_cleanup && stats.bad == 0 && stats.overwritten == 0 && stats.changed == 0 {
            let _ = tx.send(self.make_telemetry_with_tracker(
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
                &tracker,
            ));
            let _ = Self::cleanup_h2w_files(&self.config.target_dir);
        }

        let mut t = self.make_telemetry_with_tracker(
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
            &tracker,
        );
        t.first_error_offset = first_error_offset;
        t.first_error_file = first_error_file;
        t.first_error_expected = first_error_expected;
        t.first_error_actual = first_error_actual;
        let _ = tx.send(t);

        true
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
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

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn make_telemetry_with_tracker(
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
        tracker: &ProgressTracker,
    ) -> H2wTelemetry {
        let mut t = self.make_telemetry(
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
        );
        t.duration_seconds = tracker.start_time.elapsed().as_secs_f64();
        t.physiology = Some(tracker.snapshot(bytes_processed));
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_disk_full_error_precision() {
        assert!(is_disk_full_error("Direct I/O 写入失败 [Win32: 112]: 空间不足"));
        assert!(is_disk_full_error("Direct I/O 写入失败 [Win32: 39]: 空间不足"));
        assert!(is_disk_full_error("There is not enough space on the disk."));
        assert!(is_disk_full_error("disk full occurred"));
        // 确保不会因为偏移量中包含 112 或其它错误码 (如 1127/1167/19/21/23) 产生致命误报
        assert!(!is_disk_full_error("短写失败: 写入 512 字节 at offset 112000000"));
        assert!(!is_disk_full_error("Direct I/O 写入失败 [Win32: 1167]: 设备已断开连接/突发硬件拔盘"));
        assert!(!is_disk_full_error("Direct I/O 写入失败 [Win32: 1117]: 硬件 I/O 设备错误/主控无响应"));
        assert!(!is_disk_full_error("Direct I/O 写入失败 [Win32: 19]: 介质受写入保护"));
        assert!(!is_disk_full_error("Direct I/O 写入失败 [Win32: 21]: 设备未就绪"));
        assert!(!is_disk_full_error("Direct I/O 写入失败 [Win32: 23]: 数据错误 CRC"));
        assert!(!is_disk_full_error("Direct I/O 写入失败 [Win32: 1127]: 访问硬盘失败"));
    }

    #[test]
    fn test_discover_h2w_files_strict_filtering() {
        let temp_dir = std::env::temp_dir().join(format!(
            "f3_test_discover_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 创建合规测试文件
        std::fs::write(temp_dir.join("1.h2w"), b"valid_1").unwrap();
        std::fs::write(temp_dir.join("2.H2W"), b"valid_2_upper").unwrap();
        std::fs::write(temp_dir.join("10.h2w"), b"valid_10").unwrap();

        // 创建非法文件与同名文件夹，测试防呆拦截
        std::fs::write(temp_dir.join("0.h2w"), b"zero").unwrap();
        std::fs::write(temp_dir.join("01.h2w"), b"leading_zero").unwrap();
        std::fs::write(temp_dir.join("+1.h2w"), b"plus_sign").unwrap();
        std::fs::write(temp_dir.join("foo.h2w"), b"alpha").unwrap();
        std::fs::write(temp_dir.join("1.h2w.bak"), b"bak").unwrap();
        std::fs::create_dir(temp_dir.join("3.h2w")).unwrap(); // 文件夹严禁误识别

        let discovered = H2wSession::discover_h2w_files(&temp_dir);
        assert_eq!(discovered.len(), 3, "只有 1.h2w, 2.H2W, 10.h2w 应该被匹配");
        assert_eq!(discovered[0].0, 0); // 1.h2w 映射为序号 0
        assert_eq!(discovered[1].0, 1); // 2.H2W 映射为序号 1
        assert_eq!(discovered[2].0, 9); // 10.h2w 映射为序号 9

        // 测试安全一键清理：只删除匹配的文件，保留所有非法与用户文件/文件夹
        let (deleted, _freed) = H2wSession::cleanup_h2w_files(&temp_dir).unwrap();
        assert_eq!(deleted, 3);
        assert!(!temp_dir.join("1.h2w").exists());
        assert!(!temp_dir.join("2.H2W").exists());
        assert!(!temp_dir.join("10.h2w").exists());

        // 验证用户目录与非标文件完好无损未被误删
        assert!(temp_dir.join("0.h2w").exists());
        assert!(temp_dir.join("01.h2w").exists());
        assert!(temp_dir.join("+1.h2w").exists());
        assert!(temp_dir.join("foo.h2w").exists());
        assert!(temp_dir.join("1.h2w.bak").exists());
        assert!(temp_dir.join("3.h2w").is_dir());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
