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
    fn overwhelm_cache(&self, file: &mut impl BlockIo, buf: &mut AlignedBuffer, scratch_sec: u64, count: u64) {
        for i in 0..count.min(1024) {
            let target_sec = scratch_sec + i;
            F3Probe::create_probe_block(buf, target_sec ^ 0xAAAA_5555);
            let offset = target_sec * SECTOR_SIZE as u64;
            let _ = file.write_at(buf, offset);
        }
    }

    // 核心算法执行体：接收任意 BlockIo 抽象接缝 (支持生产环境 DirectIoFile 与单测环境 MemoryBlockDevice)
    // 践行「测试表面即调用表面」原则：生产与单元测试 100% 穿过相同接口
    pub fn run(&self, file: &mut impl BlockIo, tx: Sender<ProbeTelemetry>) {
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

        let total_sectors = self.config.total_sectors;
        let announced_size = total_sectors * SECTOR_SIZE as u64;

        let _ = tx.send(ProbeTelemetry {
            current_phase: "正在写入基准扇区探针...".to_string(),
            progress_pct: 0.05,
            announced_size_bytes: announced_size,
            real_size_bytes: announced_size,
            is_fake: false,
            is_finished: false,
            error: None,
        });

        // 深度复刻原版 F3：在扇区 2048 (1MB 偏移处) 写入基准参考块 (避开 MBR 分区头)
        let ref_sec = 2048u64.min(total_sectors / 4);
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

        // 冲刷主控硬件缓存 (在磁盘高位写入临时块)
        let scratch_sec = total_sectors.saturating_sub(4096);
        self.overwhelm_cache(file, &mut buf, scratch_sec, 256);

        // 步骤 2: 2^N 幂次方高位镜像回读扫描 (探测地址线回绕)
        let mut power = 4096u64; // 从 2MB 以上开始探测
        let mut wrap_detected = false;
        let mut real_capacity_sectors = total_sectors;

        while power < total_sectors {
            if self.cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            let test_sec = power + ref_sec;
            if test_sec < total_sectors {
                // 回读测试扇区：如果高位读出内容等于参考块，说明高位地址线悬空并回绕到了 ref_sec！
                if file.read_at(&mut buf, test_sec * SECTOR_SIZE as u64).is_ok() {
                    let found_sector = Self::extract_probe_sector(&buf);
                    if found_sector == Some(ref_sec) {
                        wrap_detected = true;
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
        let is_fake = wrap_detected || (real_bytes < announced_size);

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

        let final_telemetry = rx.into_iter().last().unwrap();
        assert!(final_telemetry.is_fake);
        assert_eq!(final_telemetry.real_size_bytes, real_bytes);
    }

    #[test]
    fn test_f3probe_verifies_genuine_drive() {
        // 标称 16GB，实际 16GB 正品盘
        let total_bytes = 16 * 1024 * 1024 * 1024;
        let total_sectors = total_bytes / 512;
        let mut dev = MemoryBlockDevice::new(total_bytes);

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = F3ProbeConfig {
            total_sectors,
        };
        let probe = F3Probe::new(config, cancel);

        probe.run(&mut dev, tx);

        let final_telemetry = rx.into_iter().last().unwrap();
        assert!(!final_telemetry.is_fake);
        assert_eq!(final_telemetry.real_size_bytes, total_bytes);
    }

    #[test]
    fn test_f3probe_early_cancellation() {
        let total_bytes = 16 * 1024 * 1024 * 1024;
        let total_sectors = total_bytes / 512;
        let mut dev = MemoryBlockDevice::new(total_bytes);

        let (tx, rx) = crossbeam_channel::unbounded();
        // 模拟用户在探测刚开始就点击取消
        let cancel = Arc::new(AtomicBool::new(true));
        let config = F3ProbeConfig { total_sectors };
        let probe = F3Probe::new(config, cancel);

        probe.run(&mut dev, tx);

        let msgs: Vec<_> = rx.into_iter().collect();
        assert!(!msgs.is_empty(), "必须上报消息");
        let last = msgs.last().unwrap();
        assert!(last.is_finished, "取消后必须标记为完成");
        assert!(last.error.as_deref().unwrap_or("").contains("取消"), "必须标明是用户取消");
    }
}
