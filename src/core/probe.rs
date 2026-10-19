use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;

use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::BlockIo;

pub const SECTOR_SIZE: usize = 512;
pub const BASE_PROBE_SECTOR: u64 = 2048; // 1MB 偏移处主基准探针，绝对避开 MBR / GPT 头 (LBA 0..33)
pub const AUX_PROBE_SECTOR: u64 = 4096;  // 2MB 偏移处辅助基准探针，防止主控对单个 2048 扇区实施独立 SRAM 映射欺骗
pub const MIN_SAFE_SECTORS: u64 = 8192;  // 至少需要 4MB 空间以保证基准与高位测试安全执行

/// 跳跃探针的指数跳跃阶梯 (单位: 扇区, 阶梯起点恒为 0)
/// 从 64KB 起步而非 512KB，是为了让低容量颗粒 (2MB / 3MB / 6MB) 也能被独立命中，
/// 避免首个高位采样点就落进回绕区、导致真实容量被认定为双基准位置。
const JUMP_LADDER: [u64; 21] = [
    128,
    256,
    512,
    1024,
    2048,
    4096,
    8192,
    16384,
    32768,
    65536,
    131072,
    262144,
    524288,
    1_048_576,
    2_097_152,
    4_194_304,
    8_388_608,
    16_777_216,
    33_554_432,
    67_108_864,
    268_435_456,
];

/// 二分逼近的收敛精度 (扇区)，等于 1MB
const BISECTION_TOLERANCE_SECTORS: u64 = 2048;

/// 单次缓存排空写入的块长 (扇区)，等于 64KB
const CACHE_FLUSH_BLOCK_SECTORS: u64 = 128;

/// 默认缓存排空深度 (扇区)，等于 8MB；实际深度取标称容量的 1/32 并在此区间内夹取
const CACHE_FLUSH_MIN_SECTORS: u64 = 2048;
const CACHE_FLUSH_MAX_SECTORS: u64 = 16384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeTrap {
    None,
    /// 位黑洞假盘 (超出物理容量后丢弃写入，回读全 0/全 FF 或损坏)
    BitBucket,
    /// 物理地址回绕假盘 (高位写入循环覆盖低位基准扇区)
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

    /// 生成运行时高熵会话 Salt，彻底消除任何编译期静态魔数特征 (Anti-Firmware-Fingerprint)
    fn generate_runtime_salt() -> u64 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0xA5A5_5A5A_CAFE_BABE);
        (t as u64) ^ 0x6A09E667F3BCC908 ^ 0xBB67AE8584CAA73B
    }

    /// 创建多态无特征高熵探针扇区：
    /// 前 8 字节为明文扇区号，随后 8 字节为有损单向校验字，剩余 496 字节为高熵白噪声
    fn create_probe_block(buf: &mut [u8], sector_num: u64, session_salt: u64, pattern_salt: u64) {
        let sec_bytes = sector_num.to_le_bytes();
        // 单向不可逆校验计算
        let k = (sector_num ^ session_salt ^ pattern_salt).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        let check_val = (k ^ (k >> 31)).wrapping_add(0x9E37_79B9_7F4A_7C15);

        // 只填充本扇区的 512 字节，缓冲尾部若被写入会作为高位噪声冲刷到下游扇区
        let buf = &mut buf[..SECTOR_SIZE];
        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] create sec={} len={}", sector_num, buf.len());
        }

        buf[0..8].copy_from_slice(&sec_bytes);
        buf[8..16].copy_from_slice(&check_val.to_le_bytes());

        // 剩余 496 字节填充高熵伪随机流，物理总线上表现为均匀白噪声，击穿微码特征嗅探
        let mut seed = check_val.rotate_left(17) ^ 0x517CC1B727220A95;
        for chunk in buf[16..SECTOR_SIZE].chunks_exact_mut(8) {
            seed = (seed ^ (seed >> 12)).wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(17);
            chunk.copy_from_slice(&seed.to_le_bytes());
        }
    }

    /// 核验探针扇区内容是否精确吻合
    fn verify_probe(buf: &[u8], expected_sec: u64, session_salt: u64, pattern_salt: u64) -> bool {
        if buf.len() < SECTOR_SIZE {
            return false;
        }

        let found_sec = match buf[0..8].try_into() {
            Ok(b) => u64::from_le_bytes(b),
            Err(_) => return false,
        };
        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] verify expect={} found={} first16={:02x?}", expected_sec, found_sec, &buf[..16]);
        }
        if found_sec != expected_sec {
            return false;
        }

        let found_check = match buf[8..16].try_into() {
            Ok(b) => u64::from_le_bytes(b),
            Err(_) => return false,
        };

        let k = (expected_sec ^ session_salt ^ pattern_salt).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        let expected_check = (k ^ (k >> 31)).wrapping_add(0x9E37_79B9_7F4A_7C15);
        if found_check != expected_check {
            return false;
        }

        // 进一步核验白噪声随机流（抽查关键比特段）
        let mut seed = expected_check.rotate_left(17) ^ 0x517CC1B727220A95;
        for chunk in buf[16..SECTOR_SIZE].chunks_exact(8) {
            seed = (seed ^ (seed >> 12)).wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(17);
            if chunk != seed.to_le_bytes() {
                return false;
            }
        }
        true
    }

    /// 深度缓存冲刷：使用高熵流排空主控 SRAM/DRAM 与 SLC 模拟缓存，并在循环中响应取消信号。
    /// 写入错误一律上报，绝不静默吞掉——真实硬件上排空失败等同于探测结论不可信。
    fn overwhelm_cache(
        &self,
        file: &mut impl BlockIo,
        buf: &mut AlignedBuffer,
        scratch_sec: u64,
        count: u64,
    ) -> Result<(), String> {
        let total_blocks = (count + CACHE_FLUSH_BLOCK_SECTORS - 1) / CACHE_FLUSH_BLOCK_SECTORS;

        for b in 0..total_blocks {
            if self.cancel_flag.load(Ordering::Acquire) {
                return Err("用户中止了探测任务".to_string());
            }

            let start_sec = scratch_sec + b * CACHE_FLUSH_BLOCK_SECTORS;
            let mut prng = (start_sec ^ 0xDEAD_BEEF_CAFE_BABE).wrapping_mul(6364136223846793005).wrapping_add(1);
            buf.as_mut_slice().fill(0);
            for chunk in buf.as_mut_slice().chunks_exact_mut(8) {
                prng = prng.wrapping_mul(4294967311).wrapping_add(17);
                chunk.copy_from_slice(&prng.to_le_bytes());
            }

            // 只写一个 64KB 块：整条缓冲写入会把冲刷区域放大 8 倍，越过采样点污染高位数据
            let block_bytes = buf.len().min((CACHE_FLUSH_BLOCK_SECTORS * SECTOR_SIZE as u64) as usize);
            if std::env::var("F3_DEBUG_FLUSH").is_ok() {
                eprintln!("[临时诊断] flush addr={} bytes={}", start_sec * 512, block_bytes);
            }
            file.write_at(&buf.as_slice()[..block_bytes], start_sec * 512)
                .map_err(|e| format!("缓存排空写入 LBA {} 失败: {}", start_sec, e))?;
        }
        file.flush()
            .map_err(|e| format!("缓存排空冲刷失败: {}", e))?;
        Ok(())
    }

    /// 选择本次缓存排空的起点，保证整段 [起点, 起点 + flush_depth) 严格落在采样点之前。
    /// 低容量设备上 sector - flush_depth 会下溢到 0，导致冲刷从 LBA 0 一路写到采样点，
    /// 把基准探针一起覆盖掉，因此起点必须夹在基准探针区之上。
    fn scratch_sector_for(sector: u64, flush_depth: u64) -> u64 {
        sector.saturating_sub(flush_depth).max(AUX_PROBE_SECTOR + CACHE_FLUSH_BLOCK_SECTORS)
    }

    /// 单点探针测试：写入专属探针 -> 深度排空缓存 -> 回读校验，同时复核基准探针。
    /// 返回 Some(陷阱类型) 表示该扇区不可信，None 表示通过。
    fn test_sector(
        &self,
        dev: &mut impl BlockIo,
        buf: &mut AlignedBuffer,
        sec: u64,
        session_salt: u64,
        pattern_salt: u64,
        flush_depth: u64,
    ) -> Result<Option<ProbeTrap>, String> {
        Self::create_probe_block(buf.as_mut_slice(), sec, session_salt, pattern_salt);
        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] write sec={} first16={:02x?}", sec, &buf.as_slice()[..16]);
        }
        if dev.write_at(&buf.as_slice()[..SECTOR_SIZE], sec * 512).is_err() {
            return Ok(Some(ProbeTrap::BitBucket));
        }
        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] after_write sec={} first16={:02x?}", sec, &buf.as_slice()[..16]);
        }
        if dev.flush().is_err() {
            return Ok(Some(ProbeTrap::BitBucket));
        }

        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] test_sec={} flush_start={}", sec, Self::scratch_sector_for(sec, flush_depth));
        }
        self.overwhelm_cache(dev, buf, Self::scratch_sector_for(sec, flush_depth), flush_depth)?;

        // 回读本扇区专属探针，判定位黑洞假盘
        buf.as_mut_slice().fill(0);
        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] before_read={:02x?}", &buf.as_slice()[..16]);
        }
        let rr = dev.read_at(&mut buf.as_mut_slice()[..SECTOR_SIZE], sec * 512);
        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] after_read={:02x?} ret={:?}", &buf.as_slice()[..16], rr.as_ref().map(|v| *v));
        }
        let self_ok = rr.is_ok()
            && Self::verify_probe(&buf.as_slice()[..SECTOR_SIZE], sec, session_salt, pattern_salt);

        // 回读双基准探针，判定物理地址回绕假盘
        buf.as_mut_slice().fill(0);
        let base_ok = dev.read_at(&mut buf.as_mut_slice()[..SECTOR_SIZE], BASE_PROBE_SECTOR * 512).is_ok()
            && Self::verify_probe(&buf.as_slice()[..SECTOR_SIZE], BASE_PROBE_SECTOR, session_salt, 0x55);

        buf.as_mut_slice().fill(0);
        let aux_ok = dev.read_at(&mut buf.as_mut_slice()[..SECTOR_SIZE], AUX_PROBE_SECTOR * 512).is_ok()
            && Self::verify_probe(&buf.as_slice()[..SECTOR_SIZE], AUX_PROBE_SECTOR, session_salt, 0x66);

        if std::env::var("F3_DEBUG_FLUSH").is_ok() {
            eprintln!("[临时诊断] sec={} self_ok={} base_ok={} aux_ok={}", sec, self_ok, base_ok, aux_ok);
        }
        // 基准探针被覆盖是回绕的确凿证据；仅本扇区探针失配而归属位黑洞丢弃
        if !base_ok || !aux_ok {
            return Ok(Some(ProbeTrap::WrapAround));
        }
        if !self_ok {
            return Ok(Some(ProbeTrap::BitBucket));
        }

        Ok(None)
    }

    pub fn run(&self, dev: &mut impl BlockIo, tx: Sender<ProbeTelemetry>) -> Result<ProbeResult, String> {
        let total_sectors = self.config.total_sectors;
        let announced_size = total_sectors * 512;

        // 1. 安全准入下限拦截 (至少 4MB 空间以保证基准与高位测试安全执行)
        if total_sectors < MIN_SAFE_SECTORS {
            let msg = format!("设备容量过小 ({:.2} MB)，无法执行跳跃探针探测", announced_size as f64 / (1024.0 * 1024.0));
            let _ = tx.send(ProbeTelemetry {
                progress_pct: 1.0,
                current_phase: msg.clone(),
                is_finished: true,
                is_fake: false,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                trap: ProbeTrap::None,
                error: Some(msg.clone()),
            });
            return Err(msg);
        }

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

        let mut buf = match AlignedBuffer::new(64 * 1024) {
            Ok(b) => b,
            Err(e) => {
                let err_msg = format!("分配对齐缓冲失败: {}", e);
                send_abort(&tx, err_msg.clone(), err_msg.clone());
                return Err(err_msg);
            }
        };

        let session_salt = Self::generate_runtime_salt();

        // 2. 写入双基准探针 (LBA 2048 处主基准与 LBA 4096 处辅助基准，彻底避开分区表独立 SRAM 映射)
        Self::create_probe_block(buf.as_mut_slice(), BASE_PROBE_SECTOR, session_salt, 0x55);
        if let Err(e) = dev.write_at(&buf.as_slice()[..SECTOR_SIZE], BASE_PROBE_SECTOR * 512) {
            let err_msg = format!("写入低位主基准探针失败: {}", e);
            send_abort(&tx, err_msg.clone(), err_msg.clone());
            return Err(err_msg);
        }

        Self::create_probe_block(buf.as_mut_slice(), AUX_PROBE_SECTOR, session_salt, 0x66);
        if let Err(e) = dev.write_at(&buf.as_slice()[..SECTOR_SIZE], AUX_PROBE_SECTOR * 512) {
            let err_msg = format!("写入低位辅助基准探针失败: {}", e);
            send_abort(&tx, err_msg.clone(), err_msg.clone());
            return Err(err_msg);
        }

        if let Err(e) = dev.flush() {
            let err_msg = format!("冲刷基准探针失败: {}", e);
            send_abort(&tx, err_msg.clone(), err_msg.clone());
            return Err(err_msg);
        }

        // 每个采样点前的缓存排空深度：小时取 1MB，大时封顶 8MB
        let flush_depth = (total_sectors / 16).clamp(CACHE_FLUSH_MIN_SECTORS, CACHE_FLUSH_MAX_SECTORS);

        // 3. 初始大容量高熵缓存排空，穿透主控物理缓存
        let _ = tx.send(ProbeTelemetry {
            progress_pct: 0.05,
            current_phase: "正在执行高位深度排空，穿透主控物理缓存...".to_string(),
            is_finished: false,
            is_fake: false,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            trap: ProbeTrap::None,
            error: None,
        });

        let initial_scratch = Self::scratch_sector_for(total_sectors - 1, flush_depth);
        if let Err(e) = self.overwhelm_cache(dev, &mut buf, initial_scratch, flush_depth) {
            let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
            let phase = if is_cancel {
                "探测任务已由用户手动中止".to_string()
            } else {
                format!("缓存排空失败: {}", e)
            };
            send_abort(&tx, phase, e.clone());
            return Err(e);
        }

        // 4. Phase 1: 指数跳跃阶梯粗测采样
        // 阶梯与标称容量完全解耦，低容量颗粒也会被逐级命中，杜绝「首个采样点即越界」导致的容量误判
        let mut is_counterfeit = false;
        let mut trap = ProbeTrap::None;
        let mut probes_executed = 0u64;
        let mut safe_left_sectors = AUX_PROBE_SECTOR;
        let mut suspected_right_sectors = total_sectors;
        // 标称容量尾部是否已被实测通过：未实测过就不允许直接判定为正品
        let mut tail_verified = false;

        for (idx, step) in JUMP_LADDER.iter().enumerate() {
            if self.cancel_flag.load(Ordering::Acquire) {
                send_abort(&tx, "探测任务已由用户手动中止".to_string(), "用户中止了探测任务".to_string());
                return Err("用户中止了探测任务".to_string());
            }

            let test_sec = match step.checked_add(BASE_PROBE_SECTOR) {
                Some(s) => s,
                None => break,
            };
            // 越过标称容量即停止粗测，尾部由专门的尾端核验负责
            if test_sec >= total_sectors {
                break;
            }
            let is_last = idx + 1 == JUMP_LADDER.len();

            let pct = (idx as f32 / JUMP_LADDER.len() as f32 * 0.7).clamp(0.05, 0.70);
            let _ = tx.send(ProbeTelemetry {
                progress_pct: pct,
                current_phase: format!(
                    "Phase 1 (指数探索): 测试扇区 LBA {} ({:.2} MB)...",
                    test_sec,
                    (test_sec * 512) as f64 / (1024.0 * 1024.0)
                ),
                is_finished: false,
                is_fake: false,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                trap: ProbeTrap::None,
                error: None,
            });

            probes_executed += 1;
            match self.test_sector(dev, &mut buf, test_sec, session_salt, 0xAA, flush_depth) {
                Ok(None) => {
                    // 记录已实测通过的最大安全扇区 (含该测试点自身)，作为二分区间左锚点
                    safe_left_sectors = test_sec;
                    if is_last {
                        // 阶梯已全部通过，尾部交由尾端核验判定
                        break;
                    }
                }
                Ok(Some(found_trap)) => {
                    is_counterfeit = true;
                    suspected_right_sectors = test_sec.min(total_sectors);
                    trap = found_trap;
                    break;
                }
                Err(e) => {
                    let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
                    let phase = if is_cancel {
                        "探测任务已由用户手动中止".to_string()
                    } else {
                        format!("快速排空失败: {}", e)
                    };
                    send_abort(&tx, phase, e.clone());
                    return Err(e);
                }
            }
        }

        // 阶梯耗尽仍未越界时，必须补测标称容量尾部的最后一个可写扇区。
        // 大容量盘的阶梯顶远小于标称容量，不补测就无法证明尾部真实存在，会把假盘误判为正品。
        if !is_counterfeit && !tail_verified {
            // 取末尾一个完整 64KB 块的首扇区：块对齐更贴近真实主控写入粒度，
            // 同时避开容量末尾零头扇区可能存在的保留块
            let tail_sec = total_sectors.saturating_sub(CACHE_FLUSH_BLOCK_SECTORS);
            probes_executed += 1;
            let _ = tx.send(ProbeTelemetry {
                progress_pct: 0.70,
                current_phase: format!(
                    "Phase 1 (尾部核验): 测试标称末尾扇区 LBA {} ({:.2} GB)...",
                    tail_sec,
                    (tail_sec * 512) as f64 / (1024.0 * 1024.0 * 1024.0)
                ),
                is_finished: false,
                is_fake: false,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                trap: ProbeTrap::None,
                error: None,
            });

            match self.test_sector(dev, &mut buf, tail_sec, session_salt, 0xAA, flush_depth) {
                Ok(None) => {
                    safe_left_sectors = tail_sec;
                    tail_verified = true;
                }
                Ok(Some(found_trap)) => {
                    is_counterfeit = true;
                    suspected_right_sectors = tail_sec;
                    trap = found_trap;
                }
                Err(e) => {
                    let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
                    let phase = if is_cancel {
                        "探测任务已由用户手动中止".to_string()
                    } else {
                        format!("尾部核验排空失败: {}", e)
                    };
                    send_abort(&tx, phase, e.clone());
                    return Err(e);
                }
            }
        }

        // 5. Phase 2: 二分逼近搜索 (Bisection Search)
        // 把真实物理边界收敛到 1MB 以内，攻克 3GB/6GB/12GB/24GB/48GB 等任意非二次幂颗粒
        let mut final_real_sectors = if is_counterfeit { safe_left_sectors } else { total_sectors };

        if is_counterfeit {
            let mut bisection_left = safe_left_sectors;
            let mut bisection_right = suspected_right_sectors;

            while bisection_right.saturating_sub(bisection_left) > BISECTION_TOLERANCE_SECTORS {
                if self.cancel_flag.load(Ordering::Acquire) {
                    send_abort(&tx, "探测任务已由用户手动中止".to_string(), "用户中止了探测任务".to_string());
                    return Err("用户中止了探测任务".to_string());
                }

                let mid_sec = bisection_left + ((bisection_right - bisection_left) / 2);
                let span = bisection_right.saturating_sub(bisection_left);
                let progress = 0.70 + (0.25 * (1.0 - span as f32 / total_sectors as f32));

                let _ = tx.send(ProbeTelemetry {
                    progress_pct: progress.clamp(0.70, 0.98),
                    current_phase: format!(
                        "Phase 2 (二分逼近): 正在测定边界 [LBA: {} ~ {}] 当前中点 {:.2} MB...",
                        bisection_left,
                        bisection_right,
                        (mid_sec * 512) as f64 / (1024.0 * 1024.0)
                    ),
                    is_finished: false,
                    is_fake: true,
                    announced_size_bytes: announced_size,
                    real_size_bytes: bisection_left * 512,
                    trap,
                    error: None,
                });

                probes_executed += 1;
                match self.test_sector(dev, &mut buf, mid_sec, session_salt, 0xBB, flush_depth) {
                    Ok(None) => bisection_left = mid_sec,
                    Ok(Some(_)) => bisection_right = mid_sec,
                    Err(e) => {
                        let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
                        let phase = if is_cancel {
                            "探测任务已由用户手动中止".to_string()
                        } else {
                            format!("二分逼近排空失败: {}", e)
                        };
                        send_abort(&tx, phase, e.clone());
                        return Err(e);
                    }
                }
            }

            final_real_sectors = bisection_left;
        }

        if probes_executed == 0 {
            let msg = "存储设备边界过小，未能执行有效跳跃探针".to_string();
            send_abort(&tx, msg.clone(), msg.clone());
            return Err(msg);
        }

        let real_size = final_real_sectors * 512;
        let summary_phase = if is_counterfeit {
            match trap {
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
            }
        } else {
            "探测完成：物理存储介质完全合规，未见容量虚标".to_string()
        };

        let result = ProbeResult {
            announced_size,
            real_size,
            is_fake: is_counterfeit,
            trap,
        };

        let _ = tx.send(ProbeTelemetry {
            progress_pct: 1.0,
            current_phase: summary_phase,
            is_finished: true,
            is_fake: is_counterfeit,
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
    fn test_probe_abort_during_cache_flush() {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 16384, // 8MB 虚拟设备
        };
        let probe = F3Probe::new(config, cancel_flag.clone());
        let mut dev = MemoryBlockDevice::new(8 * 1024 * 1024);
        let (tx, rx) = crossbeam_channel::unbounded();

        // 预设取消标志
        cancel_flag.store(true, Ordering::Release);

        let res = probe.run(&mut dev, tx);
        assert!(res.is_err(), "探测任务在取消标志置位时必须返回 Err");

        let mut found_finished = false;
        while let Ok(msg) = rx.try_recv() {
            if msg.is_finished {
                found_finished = true;
                assert!(msg.error.is_some(), "终止消息必须包含错误或取消原因");
            }
        }
        assert!(found_finished, "探测取消后必须向通道发送 is_finished == true 的终结通知");
    }

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
    fn test_probe_non_power_of_two_wrap_around_detection() {
        // 构造一个标称 64MB (131072 扇区)，但真实物理颗粒为非二次幂 6MB (12288 扇区) 的回绕假盘仿真设备
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 64 * 1024 * 1024 / 512, // 64MB 标称
        };
        let probe = F3Probe::new(config, cancel_flag);

        let mut dev = MemoryBlockDevice::with_fake_wrap(
            64 * 1024 * 1024,
            6 * 1024 * 1024, // 真实物理颗粒 6MB
        );

        let (tx, _rx) = crossbeam_channel::unbounded();
        let res = probe.run(&mut dev, tx).expect("探测应顺利执行完成并给出定性报告");
        eprintln!("[临时诊断] wrap6MB fake={} trap={:?} real={} announced={}", res.is_fake, res.trap, res.real_size, res.announced_size);

        // 验证 Phase 2 二分逼近能够 100% 抓捕到 6MB 假盘，绝无漏报
        assert!(res.is_fake, "6MB 非二次幂回绕盘必须被 100% 检出为假盘");
        assert_eq!(res.trap, ProbeTrap::WrapAround);
        // 实测容量应逼近 6MB 边界 (在 2048 扇区 / 1MB 容差范围内)
        let real_mb = res.real_size / (1024 * 1024);
        assert!(real_mb >= 5 && real_mb <= 7, "实测物理容量应收敛于真实 6MB 颗粒边界附近 (±1MB)，实测: {} MB", real_mb);
    }

    #[test]
    fn test_probe_honest_device_is_not_flagged() {
        // 标称容量与真实颗粒完全一致的合规正品盘，必须原样报告完整容量且不误判
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors: 64 * 1024 * 1024 / 512, // 64MB 标称
        };
        let probe = F3Probe::new(config, cancel_flag);
        let mut dev = MemoryBlockDevice::new(64 * 1024 * 1024);

        std::env::set_var("F3_DEBUG_FLUSH", "1");
        let (tx, _rx) = crossbeam_channel::unbounded();
        let res = probe.run(&mut dev, tx).expect("探测应顺利执行完成");
        eprintln!("[临时诊断] honest fake={} trap={:?} real={} announced={}", res.is_fake, res.trap, res.real_size, res.announced_size);

        assert!(!res.is_fake, "合规正品盘不得被误判为扩容盘");
        assert_eq!(res.trap, ProbeTrap::None);
        assert_eq!(res.real_size, res.announced_size, "正品盘实测容量必须等于标称容量");
    }
}
