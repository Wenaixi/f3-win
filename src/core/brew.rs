use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::core::algo::fill_sectors;
use crate::core::aligned_buf::AlignedBuffer;
use crate::core::devices::verify_target_path;
use crate::core::direct_io::DirectIoFile;

/// 延迟直方图统计分桶定义 (共 14 级区间，从微秒级至秒级)
pub const BUCKET_LABELS: [&str; 14] = [
    "< 100 µs",
    "100–250 µs",
    "250–500 µs",
    "500 µs–1 ms",
    "1–2 ms",
    "2–5 ms",
    "5–10 ms",
    "10–25 ms",
    "25–50 ms",
    "50–100 ms",
    "100–250 ms",
    "250–500 ms",
    "500 ms–1 s",
    ">= 1.0 s",
];

/// 延迟直方图统计器
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewHistogram {
    pub buckets: [u64; 14],
    pub min_duration_us: u64,
    pub max_duration_us: u64,
    pub total_duration_us: u64,
    pub sample_count: u64,
}

impl Default for BrewHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl BrewHistogram {
    pub fn new() -> Self {
        Self {
            buckets: [0; 14],
            min_duration_us: u64::MAX,
            max_duration_us: 0,
            total_duration_us: 0,
            sample_count: 0,
        }
    }

    /// 记录单次 I/O 耗时 (微秒)
    pub fn record_us(&mut self, us: u64) {
        if us < self.min_duration_us {
            self.min_duration_us = us;
        }
        if us > self.max_duration_us {
            self.max_duration_us = us;
        }
        self.total_duration_us = self.total_duration_us.saturating_add(us);
        self.sample_count += 1;

        let bucket_idx = match us {
            0..=99 => 0,
            100..=249 => 1,
            250..=499 => 2,
            500..=999 => 3,
            1000..=1999 => 4,
            2000..=4999 => 5,
            5000..=9999 => 6,
            10000..=24999 => 7,
            25000..=49999 => 8,
            50000..=99999 => 9,
            100000..=249999 => 10,
            250000..=499999 => 11,
            500000..=999999 => 12,
            _ => 13,
        };
        self.buckets[bucket_idx] += 1;
    }

    /// 平均耗时 (微秒)
    pub fn avg_duration_us(&self) -> f64 {
        if self.sample_count == 0 {
            0.0
        } else {
            self.total_duration_us as f64 / self.sample_count as f64
        }
    }

    /// 最小耗时 (毫秒)
    pub fn min_ms(&self) -> f64 {
        if self.sample_count == 0 {
            0.0
        } else {
            self.min_duration_us as f64 / 1000.0
        }
    }

    /// 最大耗时 (毫秒)
    pub fn max_ms(&self) -> f64 {
        self.max_duration_us as f64 / 1000.0
    }

    /// 平均耗时 (毫秒)
    pub fn avg_ms(&self) -> f64 {
        self.avg_duration_us() / 1000.0
    }

    /// 渲染精美的 ASCII 直方图图表
    pub fn render_ascii_chart(&self, title: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("--- {} (样本数: {}) ---\n", title, self.sample_count));
        out.push_str(&format!(
            "最小时延: {:.3} ms | 平均时延: {:.3} ms | 最大时延: {:.3} ms\n",
            self.min_ms(),
            self.avg_ms(),
            self.max_ms()
        ));
        out.push_str("------------------------------------------------------------\n");

        if self.sample_count == 0 {
            out.push_str("(无测试样本数据)\n");
            return out;
        }

        let max_count = *self.buckets.iter().max().unwrap_or(&1);
        let max_bar_width = 30;

        for (i, &count) in self.buckets.iter().enumerate() {
            let label = BUCKET_LABELS[i];
            let pct = (count as f64 / self.sample_count as f64) * 100.0;
            let bar_len = if max_count > 0 {
                ((count as f64 / max_count as f64) * max_bar_width as f64).round() as usize
            } else {
                0
            };
            let bar = "#".repeat(bar_len);
            out.push_str(&format!(
                "{:>12} | {:<30} | {:>6} ({:>5.1}%)\n",
                label, bar, count, pct
            ));
        }
        out.push_str("------------------------------------------------------------\n");
        out
    }
}

/// 延迟画像基准配置
#[derive(Debug, Clone)]
pub struct BrewConfig {
    pub target_dir: PathBuf,
    pub block_size: usize,      // 默认 64 * 1024 (64 KB)
    pub total_blocks: usize,    // 默认 1024 块 (测试 64 MB 数据量)
    pub test_read: bool,        // 是否进行读延迟比对
    pub test_filename: String,  // 测试文件名
}

impl Default for BrewConfig {
    fn default() -> Self {
        Self {
            target_dir: PathBuf::from("."),
            block_size: 64 * 1024,
            total_blocks: 1024,
            test_read: true,
            test_filename: "f3_brew_benchmark.tmp".to_string(),
        }
    }
}

/// 测试执行阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewPhase {
    Writing,
    Flushing,
    Reading,
    Completed,
}

/// 测试实时进度通知
#[derive(Debug, Clone)]
pub struct BrewProgress {
    pub phase: BrewPhase,
    pub current_block: usize,
    pub total_blocks: usize,
    pub latest_latency_us: u64,
    pub current_speed_mb_s: f64,
}

/// 延迟画像最终报告
#[derive(Debug, Clone)]
pub struct BrewResult {
    pub total_bytes: u64,
    pub block_size: usize,
    pub total_blocks: usize,
    pub write_histogram: BrewHistogram,
    pub read_histogram: Option<BrewHistogram>,
    pub write_throughput_mb_s: f64,
    pub read_throughput_mb_s: Option<f64>,
    pub slc_cache_exhaustion_detected: bool,
    pub first_chunk_avg_us: f64,
    pub last_chunk_avg_us: f64,
}

impl BrewResult {
    /// 生成可打印的完整诊断报告
    pub fn format_report(&self) -> String {
        let mut report = String::new();
        report.push_str("================ 闪存硬件 I/O 延迟画像 (f3brew) ================\n");
        let total_mb = self.total_bytes as f64 / (1024.0 * 1024.0);
        report.push_str(&format!(
            "测试总容量: {:.2} MB | 块大小: {} KB | 采样块数: {}\n",
            total_mb,
            self.block_size / 1024,
            self.total_blocks
        ));
        report.push_str(&format!(
            "写入吞吐速度: {:.2} MB/s",
            self.write_throughput_mb_s
        ));
        if let Some(r_speed) = self.read_throughput_mb_s {
            report.push_str(&format!(" | 读取吞吐速度: {:.2} MB/s", r_speed));
        }
        report.push_str("\n\n");

        if self.slc_cache_exhaustion_detected {
            report.push_str("[SLC 缓存穿透警告] 监测到写入延迟发生显著跳跃：\n");
            report.push_str(&format!(
                "  前 15% 缓存区平均时延: {:.2} ms -> 后 50% 物理颗粒平均时延: {:.2} ms (膨胀 {:.1} 倍)\n",
                self.first_chunk_avg_us / 1000.0,
                self.last_chunk_avg_us / 1000.0,
                self.last_chunk_avg_us / self.first_chunk_avg_us.max(1.0)
            ));
            report.push_str("  该特征符合闪存模拟 SLC 缓存耗尽、主控降速回写真实 TLC/QLC 颗粒的典型生理曲线。\n\n");
        }

        report.push_str(&self.write_histogram.render_ascii_chart("Direct I/O 写入延迟分布"));
        if let Some(ref rh) = self.read_histogram {
            report.push_str("\n");
            report.push_str(&rh.render_ascii_chart("Direct I/O 读取延迟分布"));
        }

        report.push_str("=================================================================\n");
        report
    }
}

/// 执行硬件延迟画像基准测试 (f3brew 原生引擎)
pub fn run_brew_benchmark<F>(
    config: &BrewConfig,
    cancelled: Arc<AtomicBool>,
    progress_fn: F,
) -> Result<BrewResult, String>
where
    F: Fn(BrewProgress),
{
    // 1. 安全硬隔离：严格拦截网络驱动器、UNC 路径与系统引导盘
    verify_target_path(&config.target_dir)?;

    let block_size = config.block_size.max(4096);
    let total_blocks = config.total_blocks.max(16);
    let total_bytes = (block_size as u64) * (total_blocks as u64);

    let test_file_path = config.target_dir.join(&config.test_filename);

    // 2. 分配 4KB 扇区物理对齐内存缓冲区
    let mut aligned_buf = AlignedBuffer::new(block_size)?;

    // 填充抗硬件压缩的非线性伪随机数据
    fill_sectors(aligned_buf.as_mut_slice(), 0, 0x5A5A_A5A5_1234_5678);

    let mut write_hist = BrewHistogram::new();
    let mut early_latencies = Vec::with_capacity(total_blocks / 6 + 1);
    let mut late_latencies = Vec::with_capacity(total_blocks / 2 + 1);

    let start_write_total = Instant::now();

    // 3. 写入阶段 (Direct I/O 硬件直通)
    {
        let mut file = DirectIoFile::create_write(&test_file_path)?;

        for i in 0..total_blocks {
            if cancelled.load(Ordering::Relaxed) {
                drop(file);
                let _ = std::fs::remove_file(&test_file_path);
                return Err("用户中断延迟画像基准测试".to_string());
            }

            let offset = (i as u64) * (block_size as u64);
            let t0 = Instant::now();
            file.write_at(aligned_buf.as_slice(), offset)?;
            let dur_us = t0.elapsed().as_micros() as u64;

            write_hist.record_us(dur_us);

            // 采样用于比对 SLC 缓存耗尽特征
            if i < total_blocks / 6 {
                early_latencies.push(dur_us);
            } else if i >= total_blocks / 2 {
                late_latencies.push(dur_us);
            }

            if i % 16 == 0 || i == total_blocks - 1 {
                let elapsed_sec = start_write_total.elapsed().as_secs_f64().max(0.0001);
                let speed_mb_s = ((i + 1) as f64 * block_size as f64) / (1024.0 * 1024.0 * elapsed_sec);
                progress_fn(BrewProgress {
                    phase: BrewPhase::Writing,
                    current_block: i + 1,
                    total_blocks,
                    latest_latency_us: dur_us,
                    current_speed_mb_s: speed_mb_s,
                });
            }
        }

        progress_fn(BrewProgress {
            phase: BrewPhase::Flushing,
            current_block: total_blocks,
            total_blocks,
            latest_latency_us: 0,
            current_speed_mb_s: 0.0,
        });

        // 下发硬件冲刷指令排空主控通道
        file.flush()?;
    }

    let write_total_sec = start_write_total.elapsed().as_secs_f64().max(0.0001);
    let write_throughput = (total_bytes as f64 / (1024.0 * 1024.0)) / write_total_sec;

    // 4. 读取阶段 (Direct I/O 硬件直通)
    let (read_hist, read_throughput) = if config.test_read {
        let mut rh = BrewHistogram::new();
        let start_read_total = Instant::now();
        let mut file = DirectIoFile::open_read(&test_file_path)?;

        for i in 0..total_blocks {
            if cancelled.load(Ordering::Relaxed) {
                drop(file);
                let _ = std::fs::remove_file(&test_file_path);
                return Err("用户中断延迟画像基准测试".to_string());
            }

            let offset = (i as u64) * (block_size as u64);
            let t0 = Instant::now();
            file.read_at(aligned_buf.as_mut_slice(), offset)?;
            let dur_us = t0.elapsed().as_micros() as u64;

            rh.record_us(dur_us);

            if i % 16 == 0 || i == total_blocks - 1 {
                let elapsed_sec = start_read_total.elapsed().as_secs_f64().max(0.0001);
                let speed_mb_s = ((i + 1) as f64 * block_size as f64) / (1024.0 * 1024.0 * elapsed_sec);
                progress_fn(BrewProgress {
                    phase: BrewPhase::Reading,
                    current_block: i + 1,
                    total_blocks,
                    latest_latency_us: dur_us,
                    current_speed_mb_s: speed_mb_s,
                });
            }
        }

        let read_total_sec = start_read_total.elapsed().as_secs_f64().max(0.0001);
        let read_speed = (total_bytes as f64 / (1024.0 * 1024.0)) / read_total_sec;
        (Some(rh), Some(read_speed))
    } else {
        (None, None)
    };

    // 5. 安全清理测试临时文件
    let _ = std::fs::remove_file(&test_file_path);

    progress_fn(BrewProgress {
        phase: BrewPhase::Completed,
        current_block: total_blocks,
        total_blocks,
        latest_latency_us: 0,
        current_speed_mb_s: 0.0,
    });

    // 6. 分析 SLC 缓存特征：若后半段平均时延较前半段膨胀 3 倍以上，判定为 SLC 缓存穿透
    let first_avg = if !early_latencies.is_empty() {
        early_latencies.iter().sum::<u64>() as f64 / early_latencies.len() as f64
    } else {
        0.0
    };
    let last_avg = if !late_latencies.is_empty() {
        late_latencies.iter().sum::<u64>() as f64 / late_latencies.len() as f64
    } else {
        0.0
    };

    let slc_exhaustion = first_avg > 0.0 && (last_avg / first_avg) >= 3.0 && last_avg >= 5000.0;

    Ok(BrewResult {
        total_bytes,
        block_size,
        total_blocks,
        write_histogram: write_hist,
        read_histogram: read_hist,
        write_throughput_mb_s: write_throughput,
        read_throughput_mb_s: read_throughput,
        slc_cache_exhaustion_detected: slc_exhaustion,
        first_chunk_avg_us: first_avg,
        last_chunk_avg_us: last_avg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_histogram_bucketing() {
        let mut hist = BrewHistogram::new();
        hist.record_us(50);      // bucket 0
        hist.record_us(150);     // bucket 1
        hist.record_us(300);     // bucket 2
        hist.record_us(800);     // bucket 3
        hist.record_us(1500);    // bucket 4
        hist.record_us(3000);    // bucket 5
        hist.record_us(8000);    // bucket 6
        hist.record_us(15000);   // bucket 7
        hist.record_us(35000);   // bucket 8
        hist.record_us(75000);   // bucket 9
        hist.record_us(150000);  // bucket 10
        hist.record_us(350000);  // bucket 11
        hist.record_us(750000);  // bucket 12
        hist.record_us(2000000); // bucket 13

        assert_eq!(hist.sample_count, 14);
        assert_eq!(hist.min_duration_us, 50);
        assert_eq!(hist.max_duration_us, 2000000);

        for (i, &cnt) in hist.buckets.iter().enumerate() {
            assert_eq!(cnt, 1, "Bucket {} 应该恰好有 1 个样本", i);
        }

        let chart = hist.render_ascii_chart("单元测试时延分布");
        assert!(chart.contains("< 100 µs"));
        assert!(chart.contains(">= 1.0 s"));
    }

    #[test]
    fn test_slc_cache_jump_detection() {
        let mut early = Vec::new();
        let mut late = Vec::new();

        // 模拟 SLC 阶段：平均 500 us (0.5 ms)
        for _ in 0..100 {
            early.push(500u64);
        }
        // 模拟 TLC/QLC 颗粒穿透阶段：平均 15000 us (15 ms, 30 倍膨胀)
        for _ in 0..100 {
            late.push(15000u64);
        }

        let first_avg = early.iter().sum::<u64>() as f64 / early.len() as f64;
        let last_avg = late.iter().sum::<u64>() as f64 / late.len() as f64;

        let slc_exhaustion = first_avg > 0.0 && (last_avg / first_avg) >= 3.0 && last_avg >= 5000.0;
        assert!(slc_exhaustion, "必须成功检出 SLC 缓存断崖穿透");
    }
}
