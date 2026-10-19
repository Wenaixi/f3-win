use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;

use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::BlockIo;

pub const SECTOR_SIZE: usize = 512;
pub const BASE_PROBE_SECTOR: u64 = 2048; // 1MB 偏移处主基准探针，绝对避开 MBR / GPT 头 (LBA 0..33)
pub const AUX_PROBE_SECTOR: u64 = 4096;  // 2MB 偏移处辅助基准探针，防止主控对单个 2048 扇区实施独立 SRAM 映射欺骗
pub const MIN_SAFE_SECTORS: u64 = 8192;  // 至少需要 4MB 空间以保证基准与高位测试安全执行

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

    /// 深度缓存冲刷：使用高熵流排空主控 SRAM/DRAM 与 SLC 模拟缓存，并在循环中响应取消信号
    fn overwhelm_cache(
        &self,
        file: &mut impl BlockIo,
        buf: &mut AlignedBuffer,
        scratch_sec: u64,
        count: u64,
    ) -> Result<(), String> {
        let block_sectors = 128u64; // 每次写入 64KB
        let total_blocks = (count + block_sectors - 1) / block_sectors;

        for b in 0..total_blocks {
            if self.cancel_flag.load(Ordering::Acquire) {
                return Err("用户中止了探测任务".to_string());
            }

            let start_sec = scratch_sec + b * block_sectors;
            let mut prng = (start_sec ^ 0xDEAD_BEEF_CAFE_BABE).wrapping_mul(6364136223846793005).wrapping_add(1);
            for chunk in buf.as_mut_slice().chunks_exact_mut(8) {
                prng = prng.wrapping_mul(4294967311).wrapping_add(17);
                chunk.copy_from_slice(&prng.to_le_bytes());
            }

            let _ = file.write_at(buf.as_slice(), start_sec * 512);
        }
        let _ = file.flush();
        Ok(())
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
        if let Err(e) = dev.write_at(&buf.as_slice()[..512], BASE_PROBE_SECTOR * 512) {
            let err_msg = format!("写入低位主基准探针失败: {}", e);
            send_abort(&tx, err_msg.clone(), err_msg.clone());
            return Err(err_msg);
        }

        Self::create_probe_block(buf.as_mut_slice(), AUX_PROBE_SECTOR, session_salt, 0x66);
        if let Err(e) = dev.write_at(&buf.as_slice()[..512], AUX_PROBE_SECTOR * 512) {
            let err_msg = format!("写入低位辅助基准探针失败: {}", e);
            send_abort(&tx, err_msg.clone(), err_msg.clone());
            return Err(err_msg);
        }

        if let Err(e) = dev.flush() {
            let err_msg = format!("冲刷基准探针失败: {}", e);
            send_abort(&tx, err_msg.clone(), err_msg.clone());
            return Err(err_msg);
        }

        // 3. 初始大容量高熵缓存排空 (最多 32MB / 65536 扇区)
        let flush_count = (total_sectors / 16).clamp(512, 65536);
        let default_scratch_sec = total_sectors.saturating_sub(flush_count + 4096);

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

        if let Err(e) = self.overwhelm_cache(dev, &mut buf, default_scratch_sec, flush_count) {
            let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
            let phase = if is_cancel {
                "探测任务已由用户手动中止".to_string()
            } else {
                format!("缓存排空失败: {}", e)
            };
            send_abort(&tx, phase, e.clone());
            return Err(e);
        }

        // 4. Phase 1: 指数跳跃粗测采样 (2MB, 4MB, 8MB, 16MB, ...)
        let mut power = 4096u64; // 从 2MB 步长开始
        let mut is_counterfeit = false;
        let mut trap = ProbeTrap::None;
        let mut probes_executed = 0u64;

        let mut safe_left_sectors = AUX_PROBE_SECTOR;
        let mut suspected_right_sectors = total_sectors;

        while power < total_sectors {
            if self.cancel_flag.load(Ordering::Acquire) {
                send_abort(&tx, "探测任务已由用户手动中止".to_string(), "用户中止了探测任务".to_string());
                return Err("用户中止了探测任务".to_string());
            }

            let test_sec = match power.checked_add(BASE_PROBE_SECTOR) {
                Some(s) => s,
                None => break,
            };

            if test_sec < total_sectors {
                probes_executed += 1;
                let pct = (power as f32 / total_sectors as f32 * 0.7).clamp(0.05, 0.70);

                let _ = tx.send(ProbeTelemetry {
                    progress_pct: pct,
                    current_phase: format!(
                        "Phase 1 (指数探索): 测试扇区 LBA {} ({:.2} GB)...",
                        test_sec,
                        (test_sec * 512) as f64 / (1024.0 * 1024.0 * 1024.0)
                    ),
                    is_finished: false,
                    is_fake: false,
                    announced_size_bytes: announced_size,
                    real_size_bytes: announced_size,
                    trap: ProbeTrap::None,
                    error: None,
                });

                // (1) 写入高位专属测试探针 (无特征高熵白噪声)
                Self::create_probe_block(buf.as_mut_slice(), test_sec, session_salt, 0xAA);
                if dev.write_at(&buf.as_slice()[..512], test_sec * 512).is_err() {
                    is_counterfeit = true;
                    suspected_right_sectors = test_sec;
                    trap = ProbeTrap::BitBucket;
                    break;
                }
                let _ = dev.flush();

                // (2) 动态安全避让高位排空
                let safe_scratch_sec = if test_sec >= default_scratch_sec && test_sec < default_scratch_sec + 2048 {
                    default_scratch_sec.saturating_sub(4096)
                } else {
                    default_scratch_sec
                };

                // 执行动态自适应冲刷
                let single_flush_sectors = 2048u64; // 提升为 1MB 高熵冲刷，增强穿透力
                if let Err(e) = self.overwhelm_cache(dev, &mut buf, safe_scratch_sec, single_flush_sectors) {
                    let is_cancel = self.cancel_flag.load(Ordering::Acquire) || e.contains("用户中止");
                    let phase = if is_cancel {
                        "探测任务已由用户手动中止".to_string()
                    } else {
                        format!("快速排空失败: {}", e)
                    };
                    send_abort(&tx, phase, e.clone());
                    return Err(e);
                }

                // (3) 高位专属探针回读核验 (绝杀位黑洞假盘 Bit-bucket)
                buf.as_mut_slice().fill(0);
                if dev.read_at(&mut buf.as_mut_slice()[..512], test_sec * 512).is_err()
                    || !Self::verify_probe(&buf.as_slice()[..512], test_sec, session_salt, 0xAA)
                {
                    is_counterfeit = true;
                    suspected_right_sectors = test_sec;
                    trap = ProbeTrap::BitBucket;
                    break;
                }

                // (4) 双基准探针回读核验 (绝杀物理地址回绕假盘 Wrap-around)
                buf.as_mut_slice().fill(0);
                let base_ok = dev.read_at(&mut buf.as_mut_slice()[..512], BASE_PROBE_SECTOR * 512).is_ok()
                    && Self::verify_probe(&buf.as_slice()[..512], BASE_PROBE_SECTOR, session_salt, 0x55);

                buf.as_mut_slice().fill(0);
                let aux_ok = dev.read_at(&mut buf.as_mut_slice()[..512], AUX_PROBE_SECTOR * 512).is_ok()
                    && Self::verify_probe(&buf.as_slice()[..512], AUX_PROBE_SECTOR, session_salt, 0x66);

                if !base_ok || !aux_ok {
                    is_counterfeit = true;
                    suspected_right_sectors = test_sec;
                    trap = ProbeTrap::WrapAround;
                    break;
                }

                safe_left_sectors = test_sec;
            }

            power = match power.checked_shl(1) {
                Some(p) => p,
                None => break,
            };
        }

        // 5. Phase 2: 二分逼近搜索 (Bisection Search)
        // 精准收敛攻克 3GB/6GB/12GB/24GB/48GB 等任意非二次幂物理颗粒边界，误差控制在 1MB (2048 扇区) 以内
        let mut final_real_sectors = if is_counterfeit { safe_left_sectors } else { total_sectors };

        if is_counterfeit && safe_left_sectors < suspected_right_sectors {
            let mut bisection_left = safe_left_sectors;
            let mut bisection_right = suspected_right_sectors;

            while (bisection_right - bisection_left) > 2048 {
                if self.cancel_flag.load(Ordering::Acquire) {
                    send_abort(&tx, "探测任务已由用户手动中止".to_string(), "用户中止了探测任务".to_string());
                    return Err("用户中止了探测任务".to_string());
                }

                let mid_sec = bisection_left + ((bisection_right - bisection_left) / 2);
                let progress = 0.70 + (0.25 * (1.0 - (bisection_right - bisection_left) as f32 / total_sectors as f32));

                let _ = tx.send(ProbeTelemetry {
                    progress_pct: progress.clamp(0.70, 0.98),
                    current_phase: format!(
                        "Phase 2 (二分逼近): 正在测定边界 [LBA: {} ~ {}] 当前中点 {:.2} GB...",
                        bisection_left,
                        bisection_right,
                        (mid_sec * 512) as f64 / (1024.0 * 1024.0 * 1024.0)
                    ),
                    is_finished: false,
                    is_fake: true,
                    announced_size_bytes: announced_size,
                    real_size_bytes: bisection_left * 512,
                    trap,
                    error: None,
                });

                // 测试 mid_sec 扇区
                Self::create_probe_block(buf.as_mut_slice(), mid_sec, session_salt, 0xBB);
                let write_ok = dev.write_at(&buf.as_slice()[..512], mid_sec * 512).is_ok();
                let _ = dev.flush();

                if write_ok {
                    let safe_scratch_sec = default_scratch_sec.saturating_sub(4096);
                    let _ = self.overwhelm_cache(dev, &mut buf, safe_scratch_sec, 2048);

                    buf.as_mut_slice().fill(0);
                    let read_ok = dev.read_at(&mut buf.as_mut_slice()[..512], mid_sec * 512).is_ok();
                    let content_ok = Self::verify_probe(&buf.as_slice()[..512], mid_sec, session_salt, 0xBB);

                    // 检查低位双基准探针完整性
                    buf.as_mut_slice().fill(0);
                    let base_intact = dev.read_at(&mut buf.as_mut_slice()[..512], BASE_PROBE_SECTOR * 512).is_ok()
                        && Self::verify_probe(&buf.as_slice()[..512], BASE_PROBE_SECTOR, session_salt, 0x55);

                    buf.as_mut_slice().fill(0);
                    let aux_intact = dev.read_at(&mut buf.as_mut_slice()[..512], AUX_PROBE_SECTOR * 512).is_ok()
                        && Self::verify_probe(&buf.as_slice()[..512], AUX_PROBE_SECTOR, session_salt, 0x66);

                    if read_ok && content_ok && base_intact && aux_intact {
                        // mid_sec 处于真实有效物理空间内
                        bisection_left = mid_sec;
                    } else {
                        // mid_sec 越界（位黑洞丢弃或发生物理回绕）
                        bisection_right = mid_sec;
                    }
                } else {
                    bisection_right = mid_sec;
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

        // 验证 Phase 2 二分逼近能够 100% 抓捕到 6MB 假盘，绝无漏报
        assert!(res.is_fake, "6MB 非二次幂回绕盘必须被 100% 检出为假盘");
        assert_eq!(res.trap, ProbeTrap::WrapAround);
        // 实测容量应逼近 6MB 边界 (在 2048 扇区 / 1MB 容差范围内)
        let real_mb = res.real_size / (1024 * 1024);
        assert!(real_mb >= 4 && real_mb <= 6, "实测物理容量应收敛于真实 6MB 颗粒边界附近，实测: {} MB", real_mb);
    }
}
