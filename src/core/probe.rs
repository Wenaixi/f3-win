use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;

use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::BlockIo;

pub const SECTOR_SIZE: usize = 512;
pub const BASE_PROBE_SECTOR: u64 = 2048; // 1MB 偏移处作为基准探针，绝对避开 MBR / GPT 头 (LBA 0..33)
pub const MIN_SAFE_SECTORS: u64 = 8192; // 至少需要 4MB 空间以保证基准与至少一阶跳跃探针有效执行

pub const PROBE_MAGIC: u64 = 0xF3_50_52_4F_42_45_01; // "F3PROBE\x01"

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

pub struct F3Probe {
    pub cancel_flag: Arc<AtomicBool>,
}

impl F3Probe {
    pub fn new(cancel_flag: Arc<AtomicBool>) -> Self {
        Self { cancel_flag }
    }

    /// 创建带校验魔数的高熵探针扇区 (使用小端序)
    fn create_probe_block(buf: &mut [u8], sector_num: u64, pattern_salt: u64) {
        // 前 24 字节：魔数 + 扇区号 + 异或校验字
        let magic = PROBE_MAGIC.to_le_bytes();
        let sec_bytes = sector_num.to_le_bytes();
        let check = (sector_num ^ pattern_salt ^ 0x5A5A_A5A5_5A5A_A5A5).to_le_bytes();

        buf[0..8].copy_from_slice(&magic);
        buf[8..16].copy_from_slice(&sec_bytes);
        buf[16..24].copy_from_slice(&check);

        // 剩余 488 字节使用高熵伪随机数据填满，彻底击穿假盘主控硬件数据压缩与折叠引擎
        let mut seed = sector_num ^ pattern_salt ^ 0x9E37_79B9_7F4A_7C15;
        for chunk in buf[24..512].chunks_exact_mut(8) {
            seed = seed.wrapping_mul(4294967311).wrapping_add(17);
            chunk.copy_from_slice(&seed.to_le_bytes());
        }
    }

    /// 核验探针扇区是否匹配特定扇区号与模式
    fn verify_probe(buf: &[u8], expected_sec: u64, pattern_salt: u64) -> bool {
        if buf.len() < SECTOR_SIZE {
            return false;
        }

        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        if magic != PROBE_MAGIC {
            return false;
        }

        let found_sec = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let found_check = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let expected_check = expected_sec ^ pattern_salt ^ 0x5A5A_A5A5_5A5A_A5A5;

        found_sec == expected_sec && found_check == expected_check
    }

    /// 提取探针扇区中的扇区号
    fn extract_probe_sector(buf: &[u8]) -> Option<u64> {
        if buf.len() < SECTOR_SIZE {
            return None;
        }
        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        if magic != PROBE_MAGIC {
            return None;
        }
        Some(u64::from_le_bytes(buf[8..16].try_into().unwrap()))
    }

    /// 深度缓存冲刷：使用高熵伪随机流排空主控 SRAM/DRAM 缓存，并在循环中高频响应取消信号
    fn overwhelm_cache(
        &self,
        file: &mut impl BlockIo,
        buf: &mut AlignedBuffer,
        scratch_sec: u64,
        count: u64,
    ) -> Result<(), String> {
        // 每次写入 1 块 64KB (128 个扇区)，大幅减少系统调用开销提升穿透吞吐
        let block_sectors = 128u64;
        let total_blocks = (count + block_sectors - 1) / block_sectors;

        for b in 0..total_blocks {
            if self.cancel_flag.load(Ordering::Acquire) {
                return Err("用户中止了探测任务".to_string());
            }

            let start_sec = scratch_sec + b * block_sectors;
            // 填充高熵排空数据
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
        let total_sectors = dev.total_sectors();
        let announced_size = total_sectors * 512;

        // 1. 安全准入下限拦截 (至少 4MB 空间以保证基准与高位测试)
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

        let mut buf = AlignedBuffer::new(64 * 1024).map_err(|e| format!("分配对齐缓冲失败: {}", e))?;

        // 2. 写入低位基准探针 (LBA 2048, 1MB 偏移处)
        Self::create_probe_block(buf.as_mut_slice(), BASE_PROBE_SECTOR, 0x55);
        dev.write_at(&buf.as_slice()[..512], BASE_PROBE_SECTOR * 512)
            .map_err(|e| format!("写入低位基准探针失败: {}", e))?;
        dev.flush().map_err(|e| format!("冲刷基准探针失败: {}", e))?;

        // 3. 初始大容量高熵缓存排空 (最多 32MB / 65536 扇区)
        let flush_count = (total_sectors / 16).clamp(512, 65536);
        let default_scratch_sec = total_sectors.saturating_sub(flush_count + 2048);

        let _ = tx.send(ProbeTelemetry {
            progress_pct: 0.1,
            current_phase: "正在执行高位深度排空，穿透主控物理缓存...".to_string(),
            is_finished: false,
            is_fake: false,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            trap: ProbeTrap::None,
            error: None,
        });

        if let Err(e) = self.overwhelm_cache(dev, &mut buf, default_scratch_sec, flush_count) {
            return Err(e);
        }

        // 4. 指数跳跃双向对流探测循环
        let mut power = 4096u64; // 从 2MB 步长开始
        let mut is_counterfeit = false;
        let mut real_capacity_sectors = total_sectors;
        let mut trap = ProbeTrap::None;
        let mut probes_executed = 0u64;

        while power < total_sectors {
            if self.cancel_flag.load(Ordering::Acquire) {
                let _ = tx.send(ProbeTelemetry {
                    progress_pct: 1.0,
                    current_phase: "探测任务已由用户手动中止".to_string(),
                    is_finished: true,
                    is_fake: false,
                    announced_size_bytes: announced_size,
                    real_size_bytes: announced_size,
                    trap: ProbeTrap::None,
                    error: Some("用户中止了探测任务".to_string()),
                });
                return Err("用户中止了探测任务".to_string());
            }

            let test_sec = match power.checked_add(BASE_PROBE_SECTOR) {
                Some(s) => s,
                None => break,
            };

            if test_sec < total_sectors {
                probes_executed += 1;
                let pct = (power as f32 / total_sectors as f32).clamp(0.1, 0.95);

                let _ = tx.send(ProbeTelemetry {
                    progress_pct: pct,
                    current_phase: format!(
                        "探测阶段: 测试扇区 LBA {} ({:.2} GB)...",
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

                // (1) 写入高位专属测试探针 (盐值 0xAA)
                Self::create_probe_block(buf.as_mut_slice(), test_sec, 0xAA);
                if dev.write_at(&buf.as_slice()[..512], test_sec * 512).is_err() {
                    // 写入即报错：物理闪存异常截断
                    is_counterfeit = true;
                    real_capacity_sectors = power;
                    trap = ProbeTrap::BitBucket;
                    break;
                }
                let _ = dev.flush();

                // (2) 关键修复：动态安全避让！杜绝排空数据覆写 test_sec 探针块引发假阳性
                let safe_scratch_sec = if test_sec >= default_scratch_sec && test_sec < default_scratch_sec + 2048 {
                    default_scratch_sec.saturating_sub(4096)
                } else {
                    default_scratch_sec
                };

                // 执行 512 扇区 (256KB) 快速高熵冲刷排空
                if let Err(e) = self.overwhelm_cache(dev, &mut buf, safe_scratch_sec, 512) {
                    return Err(e);
                }

                // (3) 高位专属探针回读核验 (绝杀位黑洞假盘 Bit-bucket)
                buf.as_mut_slice().fill(0);
                if dev.read_at(&mut buf.as_mut_slice()[..512], test_sec * 512).is_err()
                    || !Self::verify_probe(&buf.as_slice()[..512], test_sec, 0xAA)
                {
                    is_counterfeit = true;
                    real_capacity_sectors = power;
                    trap = ProbeTrap::BitBucket;
                    break;
                }

                // (4) 低位基准探针回读强恒等核验 (绝杀地址回绕假盘 Wrap-around)
                buf.as_mut_slice().fill(0);
                if dev.read_at(&mut buf.as_mut_slice()[..512], BASE_PROBE_SECTOR * 512).is_err()
                    || !Self::verify_probe(&buf.as_slice()[..512], BASE_PROBE_SECTOR, 0x55)
                {
                    is_counterfeit = true;
                    real_capacity_sectors = power;
                    trap = ProbeTrap::WrapAround;
                    break;
                }
            }

            // 安全指数推进，采用 checked_shl 杜绝溢出
            power = match power.checked_shl(1) {
                Some(p) => p,
                None => break,
            };
        }

        // 5. 校验是否执行了有效探测
        if probes_executed == 0 {
            let msg = "存储设备边界过小，未能执行有效跳跃探针".to_string();
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

        let real_size = real_capacity_sectors * 512;
        let summary_phase = if is_counterfeit {
            match trap {
                ProbeTrap::BitBucket => format!(
                    "检测到扩容盘 (类型: 位黑洞丢弃)。标称 {:.2} GB，真实可用约 {:.2} GB",
                    announced_size as f64 / (1024.0 * 1024.0 * 1024.0),
                    real_size as f64 / (1024.0 * 1024.0 * 1024.0)
                ),
                ProbeTrap::WrapAround => format!(
                    "检测到扩容盘 (类型: 物理回绕覆写)。标称 {:.2} GB，真实可用约 {:.2} GB",
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