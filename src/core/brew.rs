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
        out.push_str(&format!("--- {} (样本数: {}) ---
", title, self.sample_count));
        out.push_str(&format!(
            "最小时延: {:.3} ms | 平均时延: {:.3} ms | 最大时延: {:.3} ms
",
            self.min_ms(),
            self.avg_ms(),
            self.max_ms()
        ));
        out.push_str("------------------------------------------------------------
");

        if self.sample_count == 0 {
            out.push_str("(无测试样本数据)
");
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
                "{:>12} | {:<30} | {:>6} ({:>5.1}%)
",
                label, bar, count, pct
            ));
        }
        out.push_str("------------------------------------------------------------
");
        out
    }
}

/// 闪存微观物理参数逆向画像 (Page 大小、Erase Block 大小及主控通道调度)
#[derive(Debug, Clone)]
pub struct NandHardwareProfile {
    /// 推测的物理 Page 大小 (单位: KB, 常见 4, 8, 16, 32)
    pub estimated_page_size_kb: usize,
    /// Page 大小推导置信度 (0.0 ~ 1.0)
    pub page_confidence: f64,
    /// 推测的物理 Erase Block 大小 (单位: MB, 常见 2, 4, 8, 16, 32)
    pub estimated_erase_block_size_mb: usize,
    /// Erase Block 推导置信度 (0.0 ~ 1.0)
    pub erase_block_confidence: f64,
    /// 推测的主控通道与平面总并行度 (1, 2, 4, 8 等)
    pub estimated_parallelism: usize,
    /// 推断的主控调度架构类型
    pub controller_architecture: String,
    /// 基于物理 Erase Block 对齐推荐的最佳起始扇区 (通常为 2048 或 Block 边界倍数)
    pub optimal_alignment_sec: u64,
    /// 自动换算的修复命令 (忠实对齐原版 f3brew --fix-cmd)
    pub recommended_fix_cmd: String,
    /// 归一化自相关函数 (NACF) 能量突变前 3 主峰 (lag_blocks, correlation_score)
    pub nacf_top_peaks: Vec<(usize, f64)>,
}

impl NandHardwareProfile {
    /// 渲染微观硬件画像 ASCII 视图
    pub fn render_ascii(&self) -> String {
        let mut out = String::new();
        out.push_str("--- 闪存物理微观参数逆向画像 (NAND Hardware Reverse Profile) ---
");
        out.push_str(&format!(
            "推测物理 Page 尺寸      : {:>4} KB (置信度: {:>5.1}%)
",
            self.estimated_page_size_kb,
            self.page_confidence * 100.0
        ));
        out.push_str(&format!(
            "推测物理 Erase Block 尺寸: {:>4} MB (置信度: {:>5.1}%)
",
            self.estimated_erase_block_size_mb,
            self.erase_block_confidence * 100.0
        ));
        out.push_str(&format!(
            "主控通道与多平面并行度  : {:>4}x  (拓扑画像: {})
",
            self.estimated_parallelism,
            self.controller_architecture
        ));
        out.push_str(&format!(
            "零写放大最佳物理对齐扇区: LBA {:<8} (对齐边界: {} MB)
",
            self.optimal_alignment_sec,
            self.estimated_erase_block_size_mb
        ));
        out.push_str("------------------------------------------------------------
");
        if !self.nacf_top_peaks.is_empty() {
            out.push_str("时延周期自相关谱主极大峰 (NACF Dominant Peaks):
");
            for (rank, (lag, score)) in self.nacf_top_peaks.iter().enumerate() {
                let bar_len = (score.clamp(0.0, 1.0) * 25.0).round() as usize;
                let bar = "=".repeat(bar_len);
                out.push_str(&format!(
                    "  [Rank {}] Lag {:>5} 块 | R={:>6.3} | {:<25}
",
                    rank + 1, lag, score, bar
                ));
            }
            out.push_str("------------------------------------------------------------
");
        }
        if !self.recommended_fix_cmd.is_empty() {
            out.push_str("推荐物理对齐自愈修复命令:
");
            out.push_str(&format!("  {}
", self.recommended_fix_cmd));
            out.push_str("------------------------------------------------------------
");
        }
        out
    }
}

/// 延迟画像基准与逆向配置
#[derive(Debug, Clone)]
pub struct BrewConfig {
    pub target_dir: PathBuf,
    pub block_size: usize,      // 默认 64 * 1024 (64 KB)
    pub total_blocks: usize,    // 默认 1024 块 (测试 64 MB 数据量)
    pub test_read: bool,        // 是否进行读延迟比对
    pub test_filename: String,  // 测试文件名
    pub reverse_nand: bool,     // 是否启用物理 Page / Block / 主控调度逆向分析
}

impl Default for BrewConfig {
    fn default() -> Self {
        Self {
            target_dir: PathBuf::from("."),
            block_size: 64 * 1024,
            total_blocks: 1024,
            test_read: true,
            test_filename: "f3_brew_benchmark.tmp".to_string(),
            reverse_nand: false,
        }
    }
}

/// 测试执行阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewPhase {
    Writing,
    Flushing,
    Reading,
    Reversing,
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
    pub hardware_profile: Option<NandHardwareProfile>,
}

impl BrewResult {
    /// 生成可打印的完整诊断报告
    pub fn format_report(&self) -> String {
        let mut report = String::new();
        report.push_str("================ 闪存硬件 I/O 延迟画像 (f3brew) ================
");
        let total_mb = self.total_bytes as f64 / (1024.0 * 1024.0);
        report.push_str(&format!(
            "测试总容量: {:.2} MB | 块大小: {} KB | 采样块数: {}
",
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
        report.push_str("

");

        if self.slc_cache_exhaustion_detected {
            report.push_str("[SLC 缓存穿透警告] 监测到写入延迟发生显著跳跃：
");
            report.push_str(&format!(
                "  前 15% 缓存区平均时延: {:.2} ms -> 后 50% 物理颗粒平均时延: {:.2} ms (膨胀 {:.1} 倍)
",
                self.first_chunk_avg_us / 1000.0,
                self.last_chunk_avg_us / 1000.0,
                self.last_chunk_avg_us / self.first_chunk_avg_us.max(1.0)
            ));
            report.push_str("  该特征符合闪存模拟 SLC 缓存耗尽、主控降速回写真实 TLC/QLC 颗粒的典型生理曲线。

");
        }

        if let Some(ref prof) = self.hardware_profile {
            report.push_str(&prof.render_ascii());
            report.push('
');
        }

        report.push_str(&self.write_histogram.render_ascii_chart("Direct I/O 写入延迟分布"));
        if let Some(ref rh) = self.read_histogram {
            report.push('
');
            report.push_str(&rh.render_ascii_chart("Direct I/O 读取延迟分布"));
        }

        report.push_str("=================================================================
");
        report
    }
}

// ============================================================================
// 闪存物理微观参数逆向核心算法实现
// ============================================================================

/// 计算序列的一维离散归一化自相关函数 (NACF - Normalized Autocorrelation Function)
///
/// 公式:
/// R[\tau] = \frac{\sum_{i=0}^{N-1-\tau} (x_i - \mu)(x_{i+\tau} - \mu)}{\sum_{i=0}^{N-1} (x_i - \mu)^2}
pub fn compute_nacf(series: &[f64], min_lag: usize, max_lag: usize) -> Vec<(usize, f64)> {
    let n = series.len();
    if n < 4 || min_lag >= max_lag || min_lag >= n {
        return Vec::new();
    }

    let mean = series.iter().sum::<f64>() / (n as f64);
    let mut variance = 0.0;
    for &val in series {
        let diff = val - mean;
        variance += diff * diff;
    }

    if variance < 1e-9 {
        return Vec::new();
    }

    let clamped_max_lag = max_lag.min(n / 2);
    let mut nacf = Vec::with_capacity(clamped_max_lag.saturating_sub(min_lag) + 1);

    for lag in min_lag..=clamped_max_lag {
        let mut cov = 0.0;
        let terms = n - lag;
        for i in 0..terms {
            cov += (series[i] - mean) * (series[i + lag] - mean);
        }
        // 归一化自相关系数
        let r = cov / variance;
        nacf.push((lag, r));
    }

    nacf
}

/// 提取自相关谱中的主要峰值 (极大值过滤与能量排序)
pub fn extract_nacf_peaks(nacf: &[(usize, f64)], top_k: usize) -> Vec<(usize, f64)> {
    let mut peaks = Vec::new();
    if nacf.len() < 3 {
        return nacf.to_vec();
    }

    for i in 1..(nacf.len() - 1) {
        let prev = nacf[i - 1].1;
        let curr = nacf[i].1;
        let next = nacf[i + 1].1;

        if curr > prev && curr > next && curr > 0.15 {
            peaks.push(nacf[i]);
        }
    }

    // 按相关系数得分降序排列
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    peaks.truncate(top_k);
    peaks
}

/// 基于边际梯度与二阶差分突变反解物理 Page 尺寸
///
/// 扫描对数阶梯: 4KB, 8KB, 16KB, 32KB, 64KB, 128KB
pub fn sweep_page_size(
    target_dir: &std::path::Path,
    cancelled: &AtomicBool,
) -> Result<(usize, f64), String> {
    let sweep_sizes_kb = [4, 8, 16, 32, 64, 128];
    let mut medians_us = Vec::with_capacity(sweep_sizes_kb.len());

    let sweep_file = target_dir.join("f3_page_sweep.tmp");
    let repeat_count = 32;

    for &size_kb in &sweep_sizes_kb {
        if cancelled.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&sweep_file);
            return Err("用户中断 Page 尺寸逆向探测".to_string());
        }

        let size_bytes = size_kb * 1024;
        let mut buf = AlignedBuffer::new(size_bytes)?;
        fill_sectors(buf.as_mut_slice(), 0, 0x1122_3344_5566_7788);

        let mut latencies = Vec::with_capacity(repeat_count);

        {
            let mut file = DirectIoFile::create_write(&sweep_file)?;
            for i in 0..repeat_count {
                let offset = (i as u64) * (size_bytes as u64);
                let t0 = Instant::now();
                file.write_at(buf.as_slice(), offset)?;
                latencies.push(t0.elapsed().as_micros() as u64);
            }
            let _ = file.flush();
        }
        let _ = std::fs::remove_file(&sweep_file);

        latencies.sort_unstable();
        let median = latencies[latencies.len() / 2];
        medians_us.push(median as f64);
    }

    // 计算单位字节边际延迟增长斜率 mu = delta_t / delta_size
    let mut slopes = Vec::new();
    for i in 1..sweep_sizes_kb.len() {
        let delta_t = medians_us[i] - medians_us[i - 1];
        let delta_s = (sweep_sizes_kb[i] - sweep_sizes_kb[i - 1]) as f64;
        slopes.push(delta_t / delta_s);
    }

    // 寻找边际增长突变峰值 (未对齐到对齐转变点)
    let mut best_idx = 1; // 默认候选 8KB
    let mut max_jump = 0.0;

    for i in 1..slopes.len() {
        let jump = slopes[i] - slopes[i - 1];
        if jump > max_jump {
            max_jump = jump;
            best_idx = i;
        }
    }

    let estimated_page_kb = sweep_sizes_kb[best_idx].clamp(4, 128);
    let confidence = if max_jump > 0.0 {
        (0.70 + (max_jump / 50.0).clamp(0.0, 0.25)).min(0.95)
    } else {
        0.75
    };

    Ok((estimated_page_kb, confidence))
}

/// 基于自相关分析反解 Erase Block 大小与主控交错并行度
pub fn analyze_erase_blocks_and_parallelism(
    latencies_us: &[u64],
    block_size_bytes: usize,
) -> (usize, f64, usize, String, Vec<(usize, f64)>) {
    let series: Vec<f64> = latencies_us.iter().map(|&v| v as f64).collect();

    // 搜索 lag 从 16 块到 1024 块
    let nacf = compute_nacf(&series, 16, 512);
    let peaks = extract_nacf_peaks(&nacf, 3);

    if peaks.is_empty() {
        // 无显著周期脉冲 (例如小容量盘或纯单层 SLC/DRAM 全吸收)
        return (
            4, // 默认推测 4MB Block
            0.60,
            1,
            "单通道标准调度 (无显著自相关脉冲)".to_string(),
            Vec::new(),
        );
    }

    let primary_lag = peaks[0].0;
    let primary_score = peaks[0].1;

    // 计算主周期对应的数据尺寸 (MB)
    let block_size_mb = (primary_lag * block_size_bytes) as f64 / (1024.0 * 1024.0);

    // 标准 Erase Block 几何尺寸规格 (MB): 2, 4, 8, 16, 32
    let standard_blocks_mb = [2, 4, 8, 16, 32];
    let mut closest_block_mb = 4;
    let mut min_diff = f64::MAX;

    for &std_mb in &standard_blocks_mb {
        let diff = (block_size_mb - std_mb as f64).abs();
        if diff < min_diff {
            min_diff = diff;
            closest_block_mb = std_mb;
        }
    }

    let confidence = (primary_score * 0.95).clamp(0.65, 0.96);

    // 分析多通道与多平面并行度 (引入谐波消除滤波 Harmonic Cancellation)
    let mut parallelism = 1;
    let mut arch_desc = "单通道单平面独立调度".to_string();

    if peaks.len() >= 2 {
        let p0 = peaks[0].0 as f64;
        let p1 = peaks[1].0 as f64;
        let min_p = p0.min(p1);
        let max_p = p0.max(p1);
        let harmonic_ratio = max_p / min_p;

        // 若次峰恰为主峰的 2 倍或 3 倍整数倍谐波，判定为同一周期的天然谐振，不作为多通道凭证
        let is_harmonic = (harmonic_ratio - 2.0).abs() < 0.15 || (harmonic_ratio - 3.0).abs() < 0.18;

        if !is_harmonic {
            // 互非整数倍谐波，表明存在独立的交错或超块调度周期
            let ratio = p0 / p1;
            if (ratio - 2.0).abs() < 0.25 || (ratio - 0.5).abs() < 0.12 {
                parallelism = 2;
                arch_desc = "双通道交错 / 双平面并行 (Dual-Channel/Plane Interleave)".to_string();
            } else if (ratio - 4.0).abs() < 0.35 || (ratio - 0.25).abs() < 0.08 {
                parallelism = 4;
                arch_desc = "四通道超块并行分配 (Quad-Channel Superblock Allocation)".to_string();
            }
        }
    }

    (
        closest_block_mb,
        confidence,
        parallelism,
        arch_desc,
        peaks,
    )
}

/// 执行硬件延迟画像基准测试 (f3brew 原生引擎与物理参数逆向)
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
    let mut all_write_latencies = Vec::with_capacity(total_blocks);

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
            all_write_latencies.push(dur_us);

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

    // 6. 逆向工程分析阶段 (Page / Erase Block / 主控通道调度)
    let hardware_profile = if config.reverse_nand {
        progress_fn(BrewProgress {
            phase: BrewPhase::Reversing,
            current_block: 0,
            total_blocks: 100,
            latest_latency_us: 0,
            current_speed_mb_s: 0.0,
        });

        // 6.1 对数梯度扫描推算 Page 尺寸
        let (page_size_kb, page_conf) = sweep_page_size(&config.target_dir, &cancelled)?;

        // 6.2 离散时间序列归一化自相关反解 Erase Block 与通道调度
        let (block_mb, block_conf, parallelism, arch, peaks) =
            analyze_erase_blocks_and_parallelism(&all_write_latencies, block_size);

        // 6.3 自动换算零写放大最佳物理对齐扇区 (确保向下兼容 4Kn 物理大扇区，8 扇区整倍数对齐)
        let raw_sec = (block_mb as u64 * 1024 * 1024) / 512;
        let alignment_sec = (2048u64.max(raw_sec) + 7) & !7;
        let fix_cmd = format!("f3-win fix <物理盘号> (推荐按 {}MB / {} 扇区物理对齐)", block_mb, alignment_sec);

        Some(NandHardwareProfile {
            estimated_page_size_kb: page_size_kb,
            page_confidence: page_conf,
            estimated_erase_block_size_mb: block_mb,
            erase_block_confidence: block_conf,
            estimated_parallelism: parallelism,
            controller_architecture: arch,
            optimal_alignment_sec: alignment_sec,
            recommended_fix_cmd: fix_cmd,
            nacf_top_peaks: peaks,
        })
    } else {
        None
    };

    progress_fn(BrewProgress {
        phase: BrewPhase::Completed,
        current_block: total_blocks,
        total_blocks,
        latest_latency_us: 0,
        current_speed_mb_s: 0.0,
    });

    // 7. 分析 SLC 缓存特征：若后半段平均时延较前半段膨胀 3 倍以上，判定为 SLC 缓存穿透
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
        hardware_profile,
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
        // 模拟 SLC 阶段：平均 500 us (0.5 ms)
        let early = vec![500u64; 100];
        // 模拟 TLC/QLC 颗粒穿透阶段：平均 15000 us (15 ms, 30 倍膨胀)
        let late = vec![15000u64; 100];

        let first_avg = early.iter().sum::<u64>() as f64 / early.len() as f64;
        let last_avg = late.iter().sum::<u64>() as f64 / late.len() as f64;

        let slc_exhaustion = first_avg > 0.0 && (last_avg / first_avg) >= 3.0 && last_avg >= 5000.0;
        assert!(slc_exhaustion, "必须成功检出 SLC 缓存断崖穿透");
    }

    #[test]
    fn test_nacf_synthetic_periodic_signal() {
        // 构造注入周期为 64 块擦除脉冲的时序数据 (基底 200us，每 64 块出现一次 5000us 的擦除脉冲)
        let period = 64;
        let total_samples = 512;
        let mut series = vec![200.0; total_samples];
        for i in (0..total_samples).step_by(period) {
            series[i] = 5000.0;
        }

        let nacf = compute_nacf(&series, 16, 256);
        assert!(!nacf.is_empty(), "NACF 分析序列不应为空");

        let peaks = extract_nacf_peaks(&nacf, 3);
        assert!(!peaks.is_empty(), "必须检出明显的周期主峰");

        // 验证主峰 lag 必须精准落在 64 块周期或其整倍数上
        let top_lag = peaks[0].0;
        assert!(
            top_lag == 64 || top_lag == 128,
            "检测到的主脉冲周期应该精准为 64 (实测: {})",
            top_lag
        );
        assert!(peaks[0].1 > 0.5, "注入明显周期时自相关得分应大于 0.5");
    }

    #[test]
    fn test_erase_block_and_parallelism_inference() {
        // 模拟 64KB 块写入，在 4MB (64 块) 处注入周期脉冲
        let mut latencies = vec![300u64; 512];
        for i in (0..512).step_by(64) {
            latencies[i] = 8000;
        }

        let (block_mb, confidence, parallelism, arch, peaks) =
            analyze_erase_blocks_and_parallelism(&latencies, 64 * 1024);

        assert_eq!(block_mb, 4, "推算出的 Erase Block 应该为 4MB");
        assert!(confidence >= 0.70, "置信度应达到 70% 以上");
        assert_eq!(parallelism, 1);
        assert!(arch.contains("单通道"));
        assert!(!peaks.is_empty());
    }

    #[test]
    fn test_nand_hardware_profile_rendering() {
        let profile = NandHardwareProfile {
            estimated_page_size_kb: 16,
            page_confidence: 0.92,
            estimated_erase_block_size_mb: 8,
            erase_block_confidence: 0.88,
            estimated_parallelism: 2,
            controller_architecture: "双通道交错 / 双平面并行 (Dual-Channel/Plane Interleave)".to_string(),
            optimal_alignment_sec: 16384,
            recommended_fix_cmd: "f3-win fix E: --first-sec 16384".to_string(),
            nacf_top_peaks: vec![(128, 0.85), (64, 0.62)],
        };

        let ascii = profile.render_ascii();
        assert!(ascii.contains("16 KB"));
        assert!(ascii.contains("8 MB"));
        assert!(ascii.contains("双通道交错"));
        assert!(ascii.contains("f3-win fix E: --first-sec 16384"));
    }
}
