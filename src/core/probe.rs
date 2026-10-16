use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;

use crate::core::algo::SECTOR_SIZE;
use crate::core::aligned_buf::AlignedBuffer;
use crate::core::direct_io::BlockIo;

pub const PROBE_MAGIC: u64 = 0xF3_50_52_4F_42_45_00; // "F3PROBE\0"

#[derive(Debug, Clone)]
pub struct ProbeTelemetry {
    pub current_phase: String,
    pub progress_pct: f32,
    pub announced_size_bytes: u64,
    pub real_size_bytes: u64,
    pub is_fake: bool,
    pub is_finished: bool,
    pub error: Option<String>,
}

pub struct F3ProbeConfig {
    pub total_sectors: u64,
}

pub struct F3Probe {
    config: F3ProbeConfig,
    cancel_flag: Arc<AtomicBool>,
}

impl F3Probe {
    pub fn new(config: F3ProbeConfig, cancel_flag: Arc<AtomicBool>) -> Self {
        Self { config, cancel_flag }
    }

    // 保持向后兼容的别名方法
    pub fn run_on_device(&self, file: &mut impl BlockIo, tx: Sender<ProbeTelemetry>) {
        self.run(file, tx);
    }

    // 深度复刻 F3 原版 overwhelm_cache: 在磁盘高位区域写入缓冲块，强制排空主控 SRAM/DRAM 缓存
    // 动态扩容至最大 32MB，物理击穿现代主控的 SLC Cache
    fn overwhelm_cache(&self, file: &mut impl BlockIo, buf: &mut AlignedBuffer, scratch_sec: u64, count: u64) {
        let max_flush = count.min(65536); // 最大冲刷 65536 扇区 (32MB)
        for i in 0..max_flush {
            let target_sec = scratch_sec + i;
            Self::create_probe_block(buf, target_sec ^ 0xAAAA_5555);
            let offset = target_sec * SECTOR_SIZE as u64;
            let _ = file.write_at(buf, offset);
        }
    }

    // 核心双向对流探测算法执行体：接收任意 BlockIo 抽象接缝
    // 践行「测试表面即调用表面」原则：生产环境 DirectIoFile 与脱机单测 MemoryBlockDevice 100% 走相同代码路径
    pub fn run(&self, file: &mut impl BlockIo, tx: Sender<ProbeTelemetry>) {
        let total_sectors = self.config.total_sectors;
        let announced_size = total_sectors * SECTOR_SIZE as u64;

        // 安全熔断保护：扇区总数小于 4096 (2MB) 拒绝执行，杜绝覆写扇区 0 (MBR/GPT)
        if total_sectors < 4096 {
            let _ = tx.send(ProbeTelemetry {
                current_phase: "安全拦截：目标设备物理容量过小或无法读取".to_string(),
                progress_pct: 0.0,
                announced_size_bytes: announced_size,
                real_size_bytes: 0,
                is_fake: false,
                is_finished: true,
                error: Some("设备总扇区数不足 2MB，已安全中止以防损坏分区表".to_string()),
            });
            return;
        }

        let mut buf = match AlignedBuffer::new(SECTOR_SIZE) {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(ProbeTelemetry {
                    current_phase: "初始化失败".to_string(),
                    progress_pct: 0.0,
                    announced_size_bytes: 0,
                    real_size_bytes: 0,
                    is_fake: false,
                    is_finished: true,
                    error: Some(e),
                });
                return;
            }
        };

        let _ = tx.send(ProbeTelemetry {
            current_phase: "正在写入基准扇区探针...".to_string(),
            progress_pct: 0.05,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            is_fake: false,
            is_finished: false,
            error: None,
        });

        // 步骤 1: 在扇区 2048 (1MB 偏移处) 写入基准参考块 (避开 MBR/GPT 分区头)
        let ref_sec = 2048u64;
        Self::create_probe_block(&mut buf, ref_sec);
        if let Err(e) = file.write_at(&buf, ref_sec * SECTOR_SIZE as u64) {
            let _ = tx.send(ProbeTelemetry {
                current_phase: "写入基准探针扇区失败".to_string(),
                progress_pct: 0.0,
                announced_size_bytes: announced_size,
                real_size_bytes: 0,
                is_fake: false,
                is_finished: true,
                error: Some(e),
            });
            return;
        }

        // 冲刷主控硬件缓存 (在磁盘高位写入动态冲刷块，最大 32MB)
        let flush_count = (total_sectors / 16).clamp(256, 65536);
        let scratch_sec = total_sectors.saturating_sub(flush_count + 1024);
        self.overwhelm_cache(file, &mut buf, scratch_sec, flush_count);

        // 步骤 2: 双向对流探针扫描 (同时侦测地址回绕与位黑洞丢弃)
        let mut power = 4096u64; // 从 2MB (4096 扇区) 开始指数跳跃探测
        let mut is_counterfeit = false;
        let mut real_capacity_sectors = total_sectors;

        while power < total_sectors {
            if self.cancel_flag.load(Ordering::Relaxed) {
                let _ = tx.send(ProbeTelemetry {
                    current_phase: "探测已由用户中止".to_string(),
                    progress_pct: 1.0,
                    announced_size_bytes: announced_size,
                    real_size_bytes: 0,
                    is_fake: false,
                    is_finished: true,
                    error: Some("用户取消了探测操作".to_string()),
                });
                return;
            }

            let test_sec = power + ref_sec;
            if test_sec < total_sectors {
                // (1) 高位写入专属探针块 (带有 test_sec 的独立魔数与序号)
                Self::create_probe_block(&mut buf, test_sec);
                if file.write_at(&buf, test_sec * SECTOR_SIZE as u64).is_err() {
                    // 写入直接失败，说明物理颗粒在此前已断开或耗尽
                    is_counterfeit = true;
                    real_capacity_sectors = power;
                    break;
                }

                // (2) 高位缓存排空
                self.overwhelm_cache(file, &mut buf, scratch_sec, 256);

                // (3) 高位回读核验：防位黑洞丢弃！
                // 必须能正确读出刚才写入的 test_sec 探针；如果读出全 0、全 FF 或魔数损坏，断定为黑洞丢弃假盘！
                buf.fill(0);
                if file.read_at(&mut buf, test_sec * SECTOR_SIZE as u64).is_err() {
                    is_counterfeit = true;
                    real_capacity_sectors = power;
                    break;
                }
                let high_found = Self::extract_probe_sector(&buf);
                if high_found != Some(test_sec) {
                    is_counterfeit = true;
                    real_capacity_sectors = power;
                    break;
                }

                // (4) 低位基准扇区回读核验：防物理回绕覆盖！
                // 检查扇区 2048 是否被刚才的高位写入覆盖了；若变成了 test_sec，说明发生了高位地址线回绕！
                buf.fill(0);
                if file.read_at(&mut buf, ref_sec * SECTOR_SIZE as u64).is_ok() {
                    let ref_found = Self::extract_probe_sector(&buf);
                    if ref_found == Some(test_sec) {
                        is_counterfeit = true;
                        real_capacity_sectors = power;
                        break;
                    }
                }
            }

            let pct = (power as f32 / total_sectors as f32).min(0.95);
            let _ = tx.send(ProbeTelemetry {
                current_phase: format!("正在跳跃探测扇区 2^{}...", power.ilog2()),
                progress_pct: pct,
                announced_size_bytes: announced_size,
                real_size_bytes: announced_size,
                is_fake: false,
                is_finished: false,
                error: None,
            });

            power <<= 1;
        }

        let real_bytes = real_capacity_sectors * SECTOR_SIZE as u64;
        let is_fake = is_counterfeit || (real_bytes < announced_size);

        let _ = tx.send(ProbeTelemetry {
            current_phase: if is_fake {
                "探测完成：确认为扩容虚标假盘！"
            } else {
                "探测完成：设备容量真实合规"
            }.to_string(),
            progress_pct: 1.0,
            announced_size_bytes: announced_size,
            real_size_bytes: real_bytes,
            is_fake,
            is_finished: true,
            error: None,
        });
    }

    fn create_probe_block(buf: &mut [u8], sector_num: u64) {
        buf.fill(0);
        buf[0..8].copy_from_slice(&PROBE_MAGIC.to_ne_bytes());
        buf[8..16].copy_from_slice(&sector_num.to_ne_bytes());
        buf[16..24].copy_from_slice(&(sector_num ^ 0x5555_5555_AAAA_AAAA).to_ne_bytes());
    }

    fn extract_probe_sector(buf: &[u8]) -> Option<u64> {
        if buf.len() < 24 {
            return None;
        }
        let magic = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let sector = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        let check = u64::from_ne_bytes(buf[16..24].try_into().unwrap());
        if magic == PROBE_MAGIC && (sector ^ 0x5555_5555_AAAA_AAAA == check) {
            Some(sector)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::direct_io::MemoryBlockDevice;

    #[test]
    fn test_f3probe_detects_fake_drive_wrap_around() {
        // 标称 64GB (134,217,728 扇区)，实际物理容量 4GB (8,388,608 扇区)
        let total_sectors = 64 * 1024 * 1024 * 1024 / 512;
        let real_bytes = 4 * 1024 * 1024 * 1024;
        let mut dev = MemoryBlockDevice::with_fake_wrap(total_sectors * 512, real_bytes);

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors,
        };
        let probe = F3Probe::new(config, cancel);

        probe.run(&mut dev, tx);

        let mut final_telemetry = None;
        while let Ok(t) = rx.try_recv() {
            if t.is_finished {
                final_telemetry = Some(t);
                break;
            }
        }

        let res = final_telemetry.expect("Should produce final telemetry");
        assert!(res.is_fake, "必须准确检出扩容回绕假盘");
        assert_eq!(res.real_size_bytes, real_bytes, "真实物理容量必须识别为 4GB");
    }

    #[test]
    fn test_f3probe_detects_fake_drive_bit_bucket() {
        // 标称 32GB，真实容量 8GB，超出部分进入位黑洞 (写丢弃，读返回全 0)
        let total_sectors = 32 * 1024 * 1024 * 1024 / 512;
        let real_bytes = 8 * 1024 * 1024 * 1024;
        let mut dev = MemoryBlockDevice::with_fake_bit_bucket(total_sectors * 512, real_bytes);

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors,
        };
        let probe = F3Probe::new(config, cancel);

        probe.run(&mut dev, tx);

        let mut final_telemetry = None;
        while let Ok(t) = rx.try_recv() {
            if t.is_finished {
                final_telemetry = Some(t);
                break;
            }
        }

        let res = final_telemetry.expect("Should produce final telemetry");
        assert!(res.is_fake, "双向探针必须准确检出位黑洞丢弃型假盘");
        assert_eq!(res.real_size_bytes, real_bytes, "真实物理容量必须识别为 8GB");
    }

    #[test]
    fn test_f3probe_refuses_tiny_capacity_to_protect_mbr() {
        let total_sectors = 1000; // 仅 500KB，不足 2MB
        let mut dev = MemoryBlockDevice::new(total_sectors * 512);

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig { total_sectors };
        let probe = F3Probe::new(config, cancel);

        probe.run(&mut dev, tx);

        let mut final_telemetry = None;
        while let Ok(t) = rx.try_recv() {
            if t.is_finished {
                final_telemetry = Some(t);
                break;
            }
        }

        let res = final_telemetry.expect("Should produce final telemetry");
        assert!(res.error.is_some(), "必须拦截超小容量设备");
        assert!(res.error.unwrap().contains("分区表"));
    }
    #[test]
    fn test_f3probe_detects_genuine_drive() {
        // 标称 8GB，实际物理 8GB 真实正品盘 (完全无回绕、无黑洞)
        let total_sectors = 8 * 1024 * 1024 * 1024 / 512;
        let mut dev = MemoryBlockDevice::new(total_sectors * 512);

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig { total_sectors };
        let probe = F3Probe::new(config, cancel);

        probe.run(&mut dev, tx);

        let mut final_telemetry = None;
        while let Ok(t) = rx.try_recv() {
            if t.is_finished {
                final_telemetry = Some(t);
                break;
            }
        }

        let res = final_telemetry.expect("Should produce final telemetry");
        assert!(!res.is_fake, "真实合规正品盘绝不能被误判为假盘");
        assert_eq!(res.real_size_bytes, total_sectors * 512, "真实物理容量必须等于标称容量");
        assert!(res.error.is_none(), "正常探测不应出现错误");
    }
}
