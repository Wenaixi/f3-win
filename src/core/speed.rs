use std::time::Instant;

/// SLC 耗尽断崖特征信息
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlcCliffInfo {
    pub cliff_bytes: u64,
    pub speed_before_bps: f64,
    pub speed_after_bps: f64,
    pub drop_percentage: f64,
}

/// 存储生理特征采样快照 (用于报告与实时看板)
#[derive(Debug, Clone, Copy)]
pub struct SpeedPhysiologySnapshot {
    pub current_speed_bps: f64,
    pub peak_speed_bps: f64,
    pub valley_speed_bps: f64,
    pub average_speed_bps: f64,
    pub median_speed_bps: f64,
    pub cv_jitter: f64,
    pub diff_jitter_ratio: f64,
    pub stall_ratio: f64,
    pub slc_cliff: Option<SlcCliffInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlcStage {
    Warmup,
    InBurst,
    SuspectedCliff {
        first_detected_bytes: u64,
        consecutive_count: u8,
    },
    Exhausted,
}

/// P² (Piecewise-Parabolic) 分位数在线估算器 (中位数 p = 0.5)
/// 内存开销仅 80 字节，无需保存任何历史数组，以 O(1) 空间逼近中位数
#[derive(Debug, Clone)]
struct PSquareMedian {
    count: usize,
    q: [f64; 5],
    n: [usize; 5],
    n_prime: [f64; 5],
    dn: [f64; 5],
}

impl PSquareMedian {
    fn new() -> Self {
        Self {
            count: 0,
            q: [0.0; 5],
            n: [0; 5],
            n_prime: [1.0, 1.5, 2.0, 2.5, 3.0],
            dn: [0.0, 0.25, 0.5, 0.75, 1.0],
        }
    }

    fn add(&mut self, x: f64) {
        if self.count < 5 {
            self.q[self.count] = x;
            self.count += 1;
            if self.count == 5 {
                self.q.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                for i in 0..5 {
                    self.n[i] = i + 1;
                }
            }
            return;
        }

        let k = if x < self.q[0] {
            self.q[0] = x;
            0
        } else if x >= self.q[4] {
            self.q[4] = x;
            3
        } else {
            let mut idx = 3;
            for i in 1..4 {
                if x < self.q[i] {
                    idx = i - 1;
                    break;
                }
            }
            idx
        };

        for i in (k + 1)..5 {
            self.n[i] += 1;
        }
        for i in 0..5 {
            self.n_prime[i] += self.dn[i];
        }

        for i in 1..4 {
            let d = self.n_prime[i] - (self.n[i] as f64);
            if (d >= 1.0 && self.n[i + 1] - self.n[i] > 1)
                || (d <= -1.0 && (self.n[i] as isize) - (self.n[i - 1] as isize) > 1)
            {
                let sign = if d >= 0.0 { 1.0 } else { -1.0 };
                let d_int = sign as isize;
                let q_parabolic = self.parabolic(i, d_int);

                if self.q[i - 1] < q_parabolic && q_parabolic < self.q[i + 1] {
                    self.q[i] = q_parabolic;
                } else {
                    self.q[i] = self.linear(i, d_int);
                }
                self.n[i] = ((self.n[i] as isize) + d_int) as usize;
            }
        }
    }

    #[inline]
    fn parabolic(&self, i: usize, d: isize) -> f64 {
        let d_f = d as f64;
        let n = self.n;
        let q = self.q;
        let n_i = n[i] as f64;
        let n_prev = n[i - 1] as f64;
        let n_next = n[i + 1] as f64;

        q[i] + (d_f / (n_next - n_prev))
            * (((n_i - n_prev + d_f) * (q[i + 1] - q[i]) / (n_next - n_i))
                + ((n_next - n_i - d_f) * (q[i] - q[i - 1]) / (n_i - n_prev)))
    }

    #[inline]
    fn linear(&self, i: usize, d: isize) -> f64 {
        let d_f = d as f64;
        let n = self.n;
        let q = self.q;
        let next_idx = if d > 0 { i + 1 } else { i - 1 };
        q[i] + d_f * (q[next_idx] - q[i]) / ((n[next_idx] as isize - n[i] as isize) as f64)
    }

    #[inline]
    fn median(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else if self.count < 5 {
            let mut sorted = self.q[..self.count].to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[self.count / 2]
        } else {
            self.q[2]
        }
    }
}

/// 全维度生理速度与闪存健康特征跟踪器
#[derive(Debug, Clone)]
pub struct SpeedPhysiologyTracker {
    pub start_time: Instant,
    last_sample_time: Instant,
    last_sample_bytes: u64,
    target_bytes: u64,

    pub peak_speed_bps: f64,
    pub valley_speed_bps: f64,
    warmup_passed: bool,
    warmup_sample_count: usize,
    warmup_threshold_bytes: u64,

    sample_count: u64,
    welford_mean: f64,
    welford_m2: f64,

    last_speed: f64,
    diff_count: u64,
    diff_m2: f64,

    stall_count: u64,
    p_square: PSquareMedian,

    slc_stage: SlcStage,
    slc_burst_anchor_speed: f64,
    pub slc_cliff_info: Option<SlcCliffInfo>,
}

impl SpeedPhysiologyTracker {
    pub fn new(target_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            start_time: now,
            last_sample_time: now,
            last_sample_bytes: 0,
            target_bytes,

            peak_speed_bps: 0.0,
            valley_speed_bps: 0.0,
            warmup_passed: false,
            warmup_sample_count: 0,
            warmup_threshold_bytes: 32 * 1024 * 1024,

            sample_count: 0,
            welford_mean: 0.0,
            welford_m2: 0.0,

            last_speed: 0.0,
            diff_count: 0,
            diff_m2: 0.0,

            stall_count: 0,
            p_square: PSquareMedian::new(),

            slc_stage: SlcStage::Warmup,
            slc_burst_anchor_speed: 0.0,
            slc_cliff_info: None,
        }
    }

    pub fn sample(&mut self, current_bytes: u64) -> Option<(f64, u64, SpeedPhysiologySnapshot)> {
        let elapsed = self.last_sample_time.elapsed().as_secs_f64();
        if elapsed < 0.3 {
            return None;
        }

        let speed = (current_bytes.saturating_sub(self.last_sample_bytes)) as f64 / elapsed;
        self.last_sample_time = Instant::now();
        self.last_sample_bytes = current_bytes;

        // 1. 峰值更新
        if speed > self.peak_speed_bps {
            self.peak_speed_bps = speed;
        }

        // 2. 冷启动过滤与谷底更新
        if !self.warmup_passed {
            self.warmup_sample_count += 1;
            if current_bytes >= self.warmup_threshold_bytes || self.warmup_sample_count >= 3 {
                self.warmup_passed = true;
                self.valley_speed_bps = speed;
                self.slc_stage = SlcStage::InBurst;
                self.slc_burst_anchor_speed = self.peak_speed_bps.max(speed);
            }
        } else if speed < self.valley_speed_bps {
            self.valley_speed_bps = speed;
        }

        // 3. Welford 均值与方差
        self.sample_count += 1;
        let delta = speed - self.welford_mean;
        self.welford_mean += delta / (self.sample_count as f64);
        let delta2 = speed - self.welford_mean;
        self.welford_m2 += delta * delta2;

        // 4. 相继差抖动 (RMSSD)
        if self.sample_count > 1 {
            let diff = speed - self.last_speed;
            self.diff_count += 1;
            self.diff_m2 += diff * diff;
        }
        self.last_speed = speed;

        // 5. 卡顿统计 (< 1MB/s)
        if speed < 1_000_000.0 {
            self.stall_count += 1;
        }

        // 6. P² 中位数更新
        self.p_square.add(speed);

        // 7. SLC 断崖探测 (连续 3 周期滤波防抖)
        self.detect_slc_cliff(speed, current_bytes);

        // 8. 衍生指标计算
        let total_elapsed = self.start_time.elapsed().as_secs_f64();
        let avg_speed = if total_elapsed > 0.0 {
            current_bytes as f64 / total_elapsed
        } else {
            0.0
        };

        let variance = if self.sample_count > 1 {
            self.welford_m2 / ((self.sample_count - 1) as f64)
        } else {
            0.0
        };
        let std_dev = variance.sqrt();
        let cv_jitter = if self.welford_mean > 1e-3 {
            std_dev / self.welford_mean
        } else {
            0.0
        };

        let rmssd = if self.diff_count > 0 {
            (self.diff_m2 / (self.diff_count as f64)).sqrt()
        } else {
            0.0
        };
        let diff_jitter_ratio = if self.welford_mean > 1e-3 {
            rmssd / self.welford_mean
        } else {
            0.0
        };

        let stall_ratio = self.stall_count as f64 / self.sample_count as f64;

        let eta = if speed > 50_000.0 && self.target_bytes > current_bytes {
            ((self.target_bytes - current_bytes) as f64 / speed) as u64
        } else {
            0
        };

        let snapshot = SpeedPhysiologySnapshot {
            current_speed_bps: speed,
            peak_speed_bps: self.peak_speed_bps,
            valley_speed_bps: self.valley_speed_bps,
            average_speed_bps: avg_speed,
            median_speed_bps: self.p_square.median(),
            cv_jitter,
            diff_jitter_ratio,
            stall_ratio,
            slc_cliff: self.slc_cliff_info,
        };

        Some((speed, eta, snapshot))
    }

    fn detect_slc_cliff(&mut self, speed: f64, current_bytes: u64) {
        match self.slc_stage {
            SlcStage::InBurst => {
                if speed > self.slc_burst_anchor_speed {
                    self.slc_burst_anchor_speed = speed;
                }
                if self.slc_burst_anchor_speed > 10_000_000.0
                    && speed <= self.slc_burst_anchor_speed * 0.40
                {
                    self.slc_stage = SlcStage::SuspectedCliff {
                        first_detected_bytes: current_bytes,
                        consecutive_count: 1,
                    };
                }
            }
            SlcStage::SuspectedCliff {
                first_detected_bytes,
                mut consecutive_count,
            } => {
                if speed <= self.slc_burst_anchor_speed * 0.40 {
                    consecutive_count += 1;
                    if consecutive_count >= 3 {
                        let drop_pct = (1.0 - (speed / self.slc_burst_anchor_speed)) * 100.0;
                        self.slc_cliff_info = Some(SlcCliffInfo {
                            cliff_bytes: first_detected_bytes,
                            speed_before_bps: self.slc_burst_anchor_speed,
                            speed_after_bps: speed,
                            drop_percentage: drop_pct,
                        });
                        self.slc_stage = SlcStage::Exhausted;
                    } else {
                        self.slc_stage = SlcStage::SuspectedCliff {
                            first_detected_bytes,
                            consecutive_count,
                        };
                    }
                } else {
                    self.slc_stage = SlcStage::InBurst;
                }
            }
            SlcStage::Warmup | SlcStage::Exhausted => {}
        }
    }

    pub fn snapshot(&self, current_bytes: u64) -> SpeedPhysiologySnapshot {
        let total_elapsed = self.start_time.elapsed().as_secs_f64();
        let avg_speed = if total_elapsed > 0.0 {
            current_bytes as f64 / total_elapsed
        } else {
            0.0
        };

        let variance = if self.sample_count > 1 {
            self.welford_m2 / ((self.sample_count - 1) as f64)
        } else {
            0.0
        };
        let std_dev = variance.sqrt();
        let cv_jitter = if self.welford_mean > 1e-3 {
            std_dev / self.welford_mean
        } else {
            0.0
        };

        let rmssd = if self.diff_count > 0 {
            (self.diff_m2 / (self.diff_count as f64)).sqrt()
        } else {
            0.0
        };
        let diff_jitter_ratio = if self.welford_mean > 1e-3 {
            rmssd / self.welford_mean
        } else {
            0.0
        };

        let stall_ratio = if self.sample_count > 0 {
            self.stall_count as f64 / self.sample_count as f64
        } else {
            0.0
        };

        SpeedPhysiologySnapshot {
            current_speed_bps: self.last_speed,
            peak_speed_bps: self.peak_speed_bps,
            valley_speed_bps: self.valley_speed_bps,
            average_speed_bps: avg_speed,
            median_speed_bps: self.p_square.median(),
            cv_jitter,
            diff_jitter_ratio,
            stall_ratio,
            slc_cliff: self.slc_cliff_info,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_warmup_filters_initial_cold_start() {
        let mut tracker = SpeedPhysiologyTracker::new(1024 * 1024 * 1024);
        assert!(!tracker.warmup_passed);
        assert_eq!(tracker.valley_speed_bps, 0.0);
    }

    #[test]
    fn test_p_square_median_approximation() {
        let mut ps = PSquareMedian::new();
        for i in 1..=101 {
            ps.add(i as f64);
        }
        let med = ps.median();
        assert!((med - 51.0).abs() <= 2.0, "中位数逼近误差在允许范围内: {}", med);
    }

    #[test]
    fn test_slc_cliff_detection_with_debounce() {
        let mut tracker = SpeedPhysiologyTracker::new(10 * 1024 * 1024 * 1024);
        tracker.warmup_passed = true;
        tracker.slc_stage = SlcStage::InBurst;
        tracker.slc_burst_anchor_speed = 100_000_000.0;

        // 第 1 次掉速 (不触发假阳性)
        tracker.detect_slc_cliff(10_000_000.0, 1_000_000_000);
        assert!(tracker.slc_cliff_info.is_none());

        // 第 2 次掉速
        tracker.detect_slc_cliff(12_000_000.0, 1_010_000_000);
        assert!(tracker.slc_cliff_info.is_none());

        // 第 3 次确认断崖成立
        tracker.detect_slc_cliff(11_000_000.0, 1_020_000_000);
        assert!(tracker.slc_cliff_info.is_some());
        let cliff = tracker.slc_cliff_info.unwrap();
        assert_eq!(cliff.cliff_bytes, 1_000_000_000);
    }
}
