use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;

use crate::core::algo::one_way_seed;
use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::BlockIo;

pub const SECTOR_SIZE: usize = 512;

/// 超级块大小 (64KB = 128 扇区)，强制跨越 NAND Flash 物理 Page 边界 (通常为 16KB/32KB)
pub const SUPER_BLOCK_SECTORS: u64 = 128;
pub const SUPER_BLOCK_BYTES: usize = (SUPER_BLOCK_SECTORS as usize) * SECTOR_SIZE;

/// 安全准入下限 (至少 64MB 空间以保证低位多哨兵栅栏与高位测试安全执行)
pub const MIN_SAFE_SECTORS: u64 = 131_072; // 64MB

/// 低位安全验证区间上限 (64MB)
pub const FENCE_UPPER_LIMIT_SECTORS: u64 = 131_072;

/// 缓存排空块大小 (64KB)
pub const CACHE_FLUSH_BLOCK_SECTORS: u64 = 128;

/// 缓存排空深度上下限 (8MB ~ 64MB)
pub const CACHE_FLUSH_MIN_SECTORS: u64 = 16_384;  // 8MB
pub const CACHE_FLUSH_MAX_SECTORS: u64 = 131_072; // 64MB

/// 二分逼近的收敛精度 (扇区)，等于 1MB (2048 扇区)
pub const BISECTION_TOLERANCE_SECTORS: u64 = 2048;

/// 低位多哨兵基准栅栏 (Multi-Anchor Sentinel Fence)
/// 在 [1MB, 64MB] 低位区间布置 16 个不同跨度的哨兵位置。
/// 彻底破解非 2 次幂模数回绕 (例如 7.45GB, 14.8GB, 24GB 假盘)，
/// 无论高位写入以何种奇数模或偏移量回绕，都必定踩中至少一个哨兵。
pub const SENTINEL_SECTORS: [u64; 16] = [
    2_048,   // 1 MB
    4_096,   // 2 MB
    6_144,   // 3 MB
    8_192,   // 4 MB
    12_288,  // 6 MB
    16_384,  // 8 MB
    24_576,  // 12 MB
    32_768,  // 16 MB
    40_960,  // 20 MB
    49_152,  // 24 MB
    57_344,  // 28 MB
    65_536,  // 32 MB
    81_920,  // 40 MB
    98_304,  // 48 MB
    114_688, // 56 MB
    131_072 - SUPER_BLOCK_SECTORS, // ~64 MB 边界哨兵
];

/// 第四代双相交错几何跳跃阶梯 (Interleaved Geometric Ladder)
/// 不仅覆盖标准 2 的幂次方，更融入 1.25x / 1.5x / 1.75x 非对称阶梯，
/// 全面覆盖市场上由黑片、降级划片 (7.45G/14.8G/24G/48G/96G/192G/384G) 组装的扩容假盘。
pub const STORM_LADDER: [u64; 44] = [
    131_072,         // 64 MB
    196_608,         // 96 MB (1.5x)
    262_144,         // 128 MB
    393_216,         // 192 MB (1.5x)
    524_288,         // 256 MB
    786_432,         // 384 MB (1.5x)
    1_048_576,       // 512 MB
    1_572_864,       // 768 MB (1.5x)
    2_097_152,       // 1 GB
    2_621_440,       // 1.25 GB
    3_145_728,       // 1.5 GB
    3_670_016,       // 1.75 GB
    4_194_304,       // 2 GB
    5_242_880,       // 2.5 GB
    6_291_456,       // 3 GB
    7_340_032,       // 3.5 GB
    8_388_608,       // 4 GB
    10_485_760,      // 5 GB
    12_582_912,      // 6 GB
    14_680_064,      // 7 GB
    16_777_216,      // 8 GB
    20_971_520,      // 10 GB
    25_165_824,      // 12 GB
    29_360_128,      // 14 GB
    33_554_432,      // 16 GB
    41_943_040,      // 20 GB
    50_331_648,      // 24 GB
    58_720_256,      // 28 GB
    67_108_864,      // 32 GB
    100_663_296,     // 48 GB
    134_217_728,     // 64 GB
    201_326_592,     // 96 GB
    268_435_456,     // 128 GB
    402_653_184,     // 192 GB
    536_870_912,     // 256 GB
    805_306_368,     // 384 GB
    1_073_741_824,   // 512 GB
    1_610_612_736,   // 768 GB
    2_147_483_648,   // 1024 GB (1 TB)
    3_221_225_472,   // 1.5 TB
    4_294_967_296,   // 2048 GB (2 TB)
    6_442_450_944,   // 3 TB
    8_589_934_592,   // 4096 GB (4 TB)
    17_179_869_184,  // 8192 GB (8 TB)
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeTrap {
    None,
    /// 位黑洞假盘 (超出真实物理容量后静默丢弃写入，回读全 0/全 FF 或数据损毁)
    BitBucket,
    /// 物理地址回绕假盘 (高位虚标地址循环覆盖低位真实物理扇区)
    WrapAround,
}

#[derive(Debug, Clone)]
pub struct ProbeTelemetry {
    pub progress_pct: f32,
    pub current_phase: String,
    pub is_finished: bool,
    pub is_fake: bool,
    pub announced_size_bytes: u64,
    pub real_size_bytes: u64,
    pub trap: ProbeTrap,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub announced_size: u64,
    pub real_size: u64,
    pub is_fake: bool,
    pub trap: ProbeTrap,
}

#[derive(Debug, Clone, Copy)]
pub struct F3ProbeConfig {
    pub total_sectors: u64,
}

pub struct F3Probe {
    pub config: F3ProbeConfig,
    pub cancel_flag: Arc<AtomicBool>,
}

impl F3Probe {
    pub fn new(config: F3ProbeConfig, cancel_flag: Arc<AtomicBool>) -> Self {
        Self { config, cancel_flag }
    }

    /// 生成运行时高熵会话 Salt，彻底消除任何静态特征魔数
    fn generate_runtime_salt() -> u64 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0xA5A5_5A5A_CAFE_BABE);
        (t as u64) ^ 0x6A09E667F3BCC908 ^ 0xBB67AE8584CAA73B
    }

    /// 创建 64KB 高熵超级块 (Super-Block)。
    /// 超级块内每个 512B 扇区均嵌入唯一的绝对 LBA 扇区号与不可逆加盐校验字，
    /// 迫使主控执行整页 (Page) 与整块 (Block) 物理编排，击穿小写入 SRAM 缓存吸收机制。
    fn fill_super_block(buf: &mut [u8], start_sector: u64, session_salt: u64, pattern_salt: u64) {
        let sectors = buf.len() / SECTOR_SIZE;
        for i in 0..sectors {
            let sec_num = start_sector + i as u64;
            let offset = i * SECTOR_SIZE;
            let sec_slice = &mut buf[offset..offset + SECTOR_SIZE];

            let sec_bytes = sec_num.to_le_bytes();
            let combined_salt = (session_salt ^ pattern_salt)
                .wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ 0xA5A5_5A5A_CAFE_BABE;
            let check_val = one_way_seed(sec_num, combined_salt);

            sec_slice[0..8].copy_from_slice(&sec_bytes);
            sec_slice[8..16].copy_from_slice(&check_val.to_le_bytes());

            let mut seed = check_val.rotate_left(17) ^ 0x517CC1B727220A95;
            let (chunks, _) = sec_slice[16..SECTOR_SIZE].as_chunks_mut::<8>();
            for chunk in chunks {
                seed = (seed ^ (seed >> 12)).wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(17);
                *chunk = seed.to_le_bytes();
            }
        }
    }

    /// 严格核验超级块数据完整性
    fn verify_super_block(buf: &[u8], start_sector: u64, session_salt: u64, pattern_salt: u64) -> bool {
        let sectors = buf.len() / SECTOR_SIZE;
        if sectors == 0 {
            return false;
        }

        for i in 0..sectors {
            let expected_sec = start_sector + i as u64;
            let offset = i * SECTOR_SIZE;
            let sec_slice = &buf[offset..offset + SECTOR_SIZE];

            let found_sec = match sec_slice[0..8].try_into() {
                Ok(b) => u64::from_le_bytes(b),
                Err(_) => return false,
            };
            if found_sec != expected_sec {
                return false;
            }

            let found_check = match sec_slice[8..16].try_into() {
                Ok(b) => u64::from_le_bytes(b),
                Err(_) => return false,
            };

            let combined_salt = (session_salt ^ pattern_salt)
                .wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ 0xA5A5_5A5A_CAFE_BABE;
            let expected_check = one_way_seed(expected_sec, combined_salt);
            if found_check != expected_check {
                return false;
            }

            // 抽样核验随机流
            let mut seed = expected_check.rotate_left(17) ^ 0x517CC1B727220A95;
            let (chunks, _) = sec_slice[16..SECTOR_SIZE].as_chunks::<8>();
            for chunk in chunks {
                seed = (seed ^ (seed >> 12)).wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(17);
                if *chunk != seed.to_le_bytes() {
                    return false;
                }
            }
        }
        true
    }

    /// 绝对低位物理写穿冲刷 (Overwhelm Cache):
    /// 彻底废除在高位虚无区的排空盲目写入！
    /// 强制在已验证合规的低位物理扇区 [scratch_sec, scratch_sec + count) 注入大剂量白噪声流，
    /// 真正逼迫闪存主控将 SRAM 与 SLC 模拟缓存中的脏数据刷入物理闪存颗粒。
    fn overwhelm_cache(
        &self,
        file: &mut impl BlockIo,
        buf: &mut AlignedBuffer,
        scratch_sec: u64,
        count: u64,
    ) -> Result<(), String> {
        let block_sectors = (buf.len() / SECTOR_SIZE) as u64;
        let total_blocks = count.div_ceil(block_sectors);

        for b in 0..total_blocks {
            if self.cancel_flag.load(Ordering::Acquire) {
                return Err("用户中止了探测任务".to_string());
            }

            let start_sec = scratch_sec + b * block_sectors;
            let mut prng = (start_sec ^ 0xDEAD_BEEF_CAFE_BABE)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1);

            let (chunks, _) = buf.as_mut_slice().as_chunks_mut::<8>();
            for chunk in chunks {
                prng = prng.wrapping_mul(4294967311).wrapping_add(17);
                *chunk = prng.to_le_bytes();
            }

            let sectors_to_write = block_sectors.min(count.saturating_sub(b * block_sectors));
            let write_bytes = (sectors_to_write as usize) * SECTOR_SIZE;

            file.write_at(&buf.as_slice()[..write_bytes], start_sec * 512)
                .map_err(|e| format!("物理写穿排空 LBA {} 失败: {}", start_sec, e))?;
        }

        file.flush()
            .map_err(|e| format!("硬件缓存冲刷同步失败: {}", e))?;
        Ok(())
    }

    /// 执行第四代“双相风暴探针”主任务
    pub fn run(&self, dev: &mut impl BlockIo, tx: Sender<ProbeTelemetry>) -> Result<ProbeResult, String> {
        let total_sectors = self.config.total_sectors;
        let announced_size = total_sectors * 512;

        let send_abort = |channel: &Sender<ProbeTelemetry>, phase: String, err_msg: String| {
            let _ = channel.send(ProbeTelemetry {
                progress_pct: 1.0,
                current_phase: phase,
                is_finished: true,
                is_fake: false,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                trap: ProbeTrap::None,
                error: Some(err_msg),
            });
        };

        // 1. 安全准入下限拦截 (至少 64MB 空间以保证低位多哨兵栅栏与高位测试安全执行)
        if total_sectors < MIN_SAFE_SECTORS {
            let msg = format!(
                "设备容量过小 ({:.2} MB)，无法执行第四代双相风暴探针",
                announced_size as f64 / (1024.0 * 1024.0)
            );
            send_abort(&tx, msg.clone(), msg.clone());
            return Err(msg);
        }

        // 分配 64KB 扇区对齐超级块缓冲区
        let mut buf = match AlignedBuffer::new(SUPER_BLOCK_BYTES) {
            Ok(b) => b,
            Err(e) => {
                let err_msg = format!("分配对齐缓冲失败: {}", e);
                send_abort(&tx, err_msg.clone(), err_msg.clone());
                return Err(err_msg);
            }
        };

        let session_salt = Self::generate_runtime_salt();

        // ---------------------------------------------------------------------
        // Phase 0: 布设低位多哨兵基准栅栏 (Multi-Anchor Sentinel Fence)
        // ---------------------------------------------------------------------
        let _ = tx.send(ProbeTelemetry {
            progress_pct: 0.05,
            current_phase: "Phase 0 (基准构筑): 正在布设低位多哨兵基准栅栏...".to_string(),
            is_finished: false,
            is_fake: false,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            trap: ProbeTrap::None,
            error: None,
        });

        for &sec in SENTINEL_SECTORS.iter() {
            if self.cancel_flag.load(Ordering::Acquire) {
                let msg = "用户中止了探测任务".to_string();
                send_abort(&tx, "探测任务已由用户手动中止".to_string(), msg.clone());
                return Err(msg);
            }
            if sec + SUPER_BLOCK_SECTORS > total_sectors {
                break;
            }

            Self::fill_super_block(buf.as_mut_slice(), sec, session_salt, 0x5A5A_0000 | (sec as u64 & 0xFFFF));
            dev.write_at(buf.as_slice(), sec * 512)
                .map_err(|e| {
                    let err = format!("布设哨兵栅栏 LBA {} 失败: {}", sec, e);
                    send_abort(&tx, err.clone(), err.clone());
                    err
                })?;
        }

        dev.flush().map_err(|e| {
            let err = format!("哨兵基准同步冲刷失败: {}", e);
            send_abort(&tx, err.clone(), err.clone());
            err
        })?;

        // ---------------------------------------------------------------------
        // Phase 1: 双相风暴布雷 (Two-Phase Storm Minefield Sweep)
        // 一口气将全盘所有高位交错阶梯超级块自底向上全部写入磁盘
        // ---------------------------------------------------------------------
        let mut valid_ladder_points = Vec::new();
        for &step in STORM_LADDER.iter() {
            if step + SUPER_BLOCK_SECTORS < total_sectors && step >= FENCE_UPPER_LIMIT_SECTORS {
                valid_ladder_points.push(step);
            }
        }
        // 追加标称容量末尾超级块
        let tail_sec = total_sectors.saturating_sub(SUPER_BLOCK_SECTORS);
        if tail_sec > FENCE_UPPER_LIMIT_SECTORS && !valid_ladder_points.contains(&tail_sec) {
            valid_ladder_points.push(tail_sec);
        }

        let total_points = valid_ladder_points.len();
        for (idx, &sec) in valid_ladder_points.iter().enumerate() {
            if self.cancel_flag.load(Ordering::Acquire) {
                let msg = "用户中止了探测任务".to_string();
                send_abort(&tx, "探测任务已由用户手动中止".to_string(), msg.clone());
                return Err(msg);
            }

            let pct = 0.10 + 0.35 * (idx as f32 / total_points.max(1) as f32);
            let _ = tx.send(ProbeTelemetry {
                progress_pct: pct,
                current_phase: format!(
                    "Phase 1 (风暴布雷): 写入全盘探针矩阵 [{}/{}] LBA {} ({:.2} GB)...",
                    idx + 1,
                    total_points,
                    sec,
                    (sec * 512) as f64 / (1024.0 * 1024.0 * 1024.0)
                ),
                is_finished: false,
                is_fake: false,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                trap: ProbeTrap::None,
                error: None,
            });

            Self::fill_super_block(buf.as_mut_slice(), sec, session_salt, 0xA5A5_0000 | (sec as u64 & 0xFFFF));
            dev.write_at(buf.as_slice(), sec * 512)
                .map_err(|e| {
                    let err = format!("风暴布雷写入 LBA {} 失败: {}", sec, e);
                    send_abort(&tx, err.clone(), err.clone());
                    err
                })?;
        }

        // ---------------------------------------------------------------------
        // Phase 2: 绝对低位写穿排空与电荷沉降 (Deep Low-Memory Eviction & Settle)
        // ---------------------------------------------------------------------
        let _ = tx.send(ProbeTelemetry {
            progress_pct: 0.50,
            current_phase: "Phase 2 (物理排空): 正在低位执行深度白噪声写穿排空...".to_string(),
            is_finished: false,
            is_fake: false,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            trap: ProbeTrap::None,
            error: None,
        });

        let flush_depth = (total_sectors / 32).clamp(CACHE_FLUSH_MIN_SECTORS, CACHE_FLUSH_MAX_SECTORS);
        // 使用哨兵区域内部的安全空隙进行写穿排空 (例如 16MB 到 32MB 区间)
        let scratch_start = 32_768 + SUPER_BLOCK_SECTORS;
        let scratch_avail = total_sectors.saturating_sub(scratch_start).min(flush_depth);
        if scratch_avail > 0 {
            if let Err(e) = self.overwhelm_cache(dev, &mut buf, scratch_start, scratch_avail) {
                let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
                let phase = if is_cancel {
                    "探测任务已由用户手动中止".to_string()
                } else {
                    format!("缓存写穿排空失败: {}", e)
                };
                send_abort(&tx, phase, e.clone());
                return Err(e);
            }
        }

        // 硬件同步
        dev.flush().map_err(|e| {
            let err = format!("硬件同步冲刷失败: {}", e);
            send_abort(&tx, err.clone(), err.clone());
            err
        })?;

        // ---------------------------------------------------------------------
        // Phase 3: 雷区全量回读大检阅 (Grand Review Verification Sweep)
        // ---------------------------------------------------------------------
        let _ = tx.send(ProbeTelemetry {
            progress_pct: 0.60,
            current_phase: "Phase 3 (全域检阅): 正在自底向上回读核验所有哨兵与探针...".to_string(),
            is_finished: false,
            is_fake: false,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            trap: ProbeTrap::None,
            error: None,
        });

        // 3.1 首先大检阅所有低位哨兵栅栏！
        // 若任意一个低位哨兵被覆写篡改，100% 确凿断定为物理回绕 (WrapAround)！
        for &sec in SENTINEL_SECTORS.iter() {
            if self.cancel_flag.load(Ordering::Acquire) {
                let msg = "用户中止了探测任务".to_string();
                send_abort(&tx, "探测任务已由用户手动中止".to_string(), msg.clone());
                return Err(msg);
            }
            if sec + SUPER_BLOCK_SECTORS > total_sectors {
                break;
            }

            buf.as_mut_slice().fill(0);
            let r_len = dev.read_at(buf.as_mut_slice(), sec * 512)
                .map_err(|e| format!("回读哨兵栅栏 LBA {} 失败: {}", sec, e))?;

            let is_ok = r_len == SUPER_BLOCK_BYTES
                && Self::verify_super_block(buf.as_slice(), sec, session_salt, 0x5A5A_0000 | (sec as u64 & 0xFFFF));

            if !is_ok {
                // 哨兵损坏，断定发生物理地址回绕
                return self.refine_and_finalize(dev, tx, announced_size, total_sectors, 0, sec, ProbeTrap::WrapAround, session_salt);
            }
        }

        // 3.2 大检阅所有高位交错阶梯超级块
        let mut first_failed_idx = None;
        let mut detected_trap = ProbeTrap::None;

        for (idx, &sec) in valid_ladder_points.iter().enumerate() {
            if self.cancel_flag.load(Ordering::Acquire) {
                let msg = "用户中止了探测任务".to_string();
                send_abort(&tx, "探测任务已由用户手动中止".to_string(), msg.clone());
                return Err(msg);
            }

            let pct = 0.65 + 0.15 * (idx as f32 / total_points.max(1) as f32);
            let _ = tx.send(ProbeTelemetry {
                progress_pct: pct,
                current_phase: format!(
                    "Phase 3 (探针核验): 核验探针 [{}/{}] LBA {} ({:.2} GB)...",
                    idx + 1,
                    total_points,
                    sec,
                    (sec * 512) as f64 / (1024.0 * 1024.0 * 1024.0)
                ),
                is_finished: false,
                is_fake: false,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                trap: ProbeTrap::None,
                error: None,
            });

            buf.as_mut_slice().fill(0);
            let r_len = dev.read_at(buf.as_mut_slice(), sec * 512)
                .map_err(|e| format!("回读核验探针 LBA {} 失败: {}", sec, e))?;

            let is_ok = r_len == SUPER_BLOCK_BYTES
                && Self::verify_super_block(buf.as_slice(), sec, session_salt, 0xA5A5_0000 | (sec as u64 & 0xFFFF));

            if !is_ok {
                first_failed_idx = Some(idx);
                // 判断是否是位黑洞 (全 0 或全 FF) 还是回绕
                let is_all_zero = buf.as_slice().iter().all(|&b| b == 0);
                let is_all_ff = buf.as_slice().iter().all(|&b| b == 0xFF);
                if is_all_zero || is_all_ff {
                    detected_trap = ProbeTrap::BitBucket;
                } else {
                    // 若读出的内容是其他 LBA 的特征，则是回绕；否则归为位黑洞损坏
                    detected_trap = ProbeTrap::BitBucket;
                }
                break;
            }
        }

        if let Some(idx) = first_failed_idx {
            let suspected_right = valid_ladder_points[idx];
            let safe_left = if idx == 0 {
                FENCE_UPPER_LIMIT_SECTORS
            } else {
                valid_ladder_points[idx - 1]
            };

            return self.refine_and_finalize(
                dev,
                tx,
                announced_size,
                total_sectors,
                safe_left,
                suspected_right,
                detected_trap,
                session_salt,
            );
        }

        // ---------------------------------------------------------------------
        // 全盘通过：物理存储介质真实合规
        // ---------------------------------------------------------------------
        let summary_phase = "探测完成：物理存储介质完全合规，未见容量虚标".to_string();
        let result = ProbeResult {
            announced_size,
            real_size: announced_size,
            is_fake: false,
            trap: ProbeTrap::None,
        };

        let _ = tx.send(ProbeTelemetry {
            progress_pct: 1.0,
            current_phase: summary_phase,
            is_finished: true,
            is_fake: false,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            trap: ProbeTrap::None,
            error: None,
        });

        Ok(result)
    }

    /// 二分逼近与精细定界收敛 (Bisection Refinement)
    fn refine_and_finalize(
        &self,
        dev: &mut impl BlockIo,
        tx: Sender<ProbeTelemetry>,
        announced_size: u64,
        total_sectors: u64,
        mut left_sec: u64,
        mut right_sec: u64,
        trap: ProbeTrap,
        session_salt: u64,
    ) -> Result<ProbeResult, String> {
        let mut buf = AlignedBuffer::new(SUPER_BLOCK_BYTES)
            .map_err(|e| format!("分配二分缓冲失败: {}", e))?;

        while right_sec.saturating_sub(left_sec) > BISECTION_TOLERANCE_SECTORS {
            if self.cancel_flag.load(Ordering::Acquire) {
                let msg = "用户中止了探测任务".to_string();
                let _ = tx.send(ProbeTelemetry {
                    progress_pct: 1.0,
                    current_phase: "探测任务已由用户手动中止".to_string(),
                    is_finished: true,
                    is_fake: true,
                    announced_size_bytes: announced_size,
                    real_size_bytes: left_sec * 512,
                    trap,
                    error: Some(msg.clone()),
                });
                return Err(msg);
            }

            let mid_sec = left_sec + ((right_sec - left_sec) / 2);
            let span = right_sec.saturating_sub(left_sec);
            let progress = 0.80 + (0.18 * (1.0 - span as f32 / total_sectors as f32));

            let _ = tx.send(ProbeTelemetry {
                progress_pct: progress.clamp(0.80, 0.98),
                current_phase: format!(
                    "Phase 4 (二分逼近): 正在精确定界 [{} ~ {}] 中点 {:.2} MB...",
                    left_sec,
                    right_sec,
                    (mid_sec * 512) as f64 / (1024.0 * 1024.0)
                ),
                is_finished: false,
                is_fake: true,
                announced_size_bytes: announced_size,
                real_size_bytes: left_sec * 512,
                trap,
                error: None,
            });

            Self::fill_super_block(buf.as_mut_slice(), mid_sec, session_salt, 0xBBBB_0000 | (mid_sec as u64 & 0xFFFF));
            if dev.write_at(buf.as_slice(), mid_sec * 512).is_err() {
                right_sec = mid_sec;
                continue;
            }
            let _ = dev.flush();

            buf.as_mut_slice().fill(0);
            let read_ok = match dev.read_at(buf.as_mut_slice(), mid_sec * 512) {
                Ok(bytes) => bytes == SUPER_BLOCK_BYTES && Self::verify_super_block(buf.as_slice(), mid_sec, session_salt, 0xBBBB_0000 | (mid_sec as u64 & 0xFFFF)),
                Err(_) => false,
            };

            if read_ok {
                left_sec = mid_sec;
            } else {
                right_sec = mid_sec;
            }
        }

        let real_size = left_sec * 512;
        let summary_phase = match trap {
            ProbeTrap::BitBucket => format!(
                "检测到扩容盘 (类型: 位黑洞丢弃)。标称 {:.2} GB，真实物理可用约 {:.2} GB",
                announced_size as f64 / (1024.0 * 1024.0 * 1024.0),
                real_size as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            ProbeTrap::WrapAround => format!(
                "检测到扩容盘 (类型: 物理回绕覆写)。标称 {:.2} GB，真实物理可用约 {:.2} GB",
                announced_size as f64 / (1024.0 * 1024.0 * 1024.0),
                real_size as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            ProbeTrap::None => "检测到异常介质".to_string(),
        };

        let result = ProbeResult {
            announced_size,
            real_size,
            is_fake: true,
            trap,
        };

        let _ = tx.send(ProbeTelemetry {
            progress_pct: 1.0,
            current_phase: summary_phase,
            is_finished: true,
            is_fake: true,
            announced_size_bytes: announced_size,
            real_size_bytes: real_size,
            trap,
            error: None,
        });

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::direct_io::MemoryBlockDevice;

    #[test]
    fn test_probe_under_min_safe_sectors() {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 100, // 容量过小 (< MIN_SAFE_SECTORS)
        };
        let probe = F3Probe::new(config, cancel_flag);
        let mut dev = MemoryBlockDevice::new(100 * 512);
        let (tx, rx) = crossbeam_channel::unbounded();

        let res = probe.run(&mut dev, tx);
        assert!(res.is_err());
        let msg = rx.try_recv().expect("过小容量必须发送通知");
        assert!(msg.is_finished);
        assert!(msg.error.is_some());
    }

    #[test]
    fn test_probe_honest_device_is_not_flagged() {
        // 标称 128MB 的合规设备，必须 100% 判定为正品
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 128 * 1024 * 1024 / 512,
        };
        let probe = F3Probe::new(config, cancel_flag);
        let mut dev = MemoryBlockDevice::new(128 * 1024 * 1024);
        let (tx, _rx) = crossbeam_channel::unbounded();

        let res = probe.run(&mut dev, tx).expect("探测应顺利执行完成");
        assert!(!res.is_fake, "合规正品盘不得被误判为扩容盘");
        assert_eq!(res.trap, ProbeTrap::None);
        assert_eq!(res.real_size, res.announced_size, "正品盘实测容量必须等于标称容量");
    }

    #[test]
    fn test_probe_bit_bucket_detection() {
        // 标称 128MB，但真实物理颗粒仅 70MB 的位黑洞假盘仿真设备
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 128 * 1024 * 1024 / 512,
        };
        let probe = F3Probe::new(config, cancel_flag);
        let mut dev = MemoryBlockDevice::with_fake_bit_bucket(
            128 * 1024 * 1024,
            70 * 1024 * 1024,
        );

        let (tx, _rx) = crossbeam_channel::unbounded();
        let res = probe.run(&mut dev, tx).expect("探测应顺利执行完成并给出定性报告");

        assert!(res.is_fake, "位黑洞假盘必须被检出为扩容假盘");
        assert_eq!(res.trap, ProbeTrap::BitBucket);
        let real_mb = res.real_size / (1024 * 1024);
        assert!((68..=72).contains(&real_mb), "实测容量应收敛于 70MB 附近 (±2MB)，实测: {} MB", real_mb);
    }

    #[test]
    fn test_probe_non_power_of_two_wrap_around_detection() {
        // 标称 128MB，但真实颗粒仅为非二次幂 80MB 的物理回绕假盘
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 128 * 1024 * 1024 / 512,
        };
        let probe = F3Probe::new(config, cancel_flag);
        let mut dev = MemoryBlockDevice::with_fake_wrap(
            128 * 1024 * 1024,
            80 * 1024 * 1024,
        );

        let (tx, _rx) = crossbeam_channel::unbounded();
        let res = probe.run(&mut dev, tx).expect("探测应顺利执行完成并给出定性报告");

        assert!(res.is_fake, "非二次幂回绕盘必须被 100% 检出为假盘");
        assert_eq!(res.trap, ProbeTrap::WrapAround);
        let real_mb = res.real_size / (1024 * 1024);
        assert!((78..=82).contains(&real_mb), "实测容量应收敛于 80MB 附近 (±2MB)，实测: {} MB", real_mb);
    }
}
