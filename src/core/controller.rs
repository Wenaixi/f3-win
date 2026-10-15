use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use crossbeam_channel::{unbounded, Receiver};

use crate::core::algo::{BlockState, BlockStats};
use crate::core::devices::StorageTarget;
use crate::core::fix::F3FixPlan;
use crate::core::probe::{F3Probe, F3ProbeConfig, ProbeTelemetry};
use crate::core::session::{H2wConfig, H2wMode, H2wPhase, H2wSession, H2wTelemetry};

// 任务运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Idle,
    Running,
    Finished,
    Error,
}

// 任务类型种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Idle,
    Write,
    Verify,
    WriteAndVerify,
    Probe,
}

// 任务聚合只读快照 (TaskSnapshot)
// UI 界面在每帧仅消费此快照，实现完全无状态的纯净视图渲染
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub status_message: String,
    pub current_speed_mb: f64,
    pub speed_history: Vec<[f64; 2]>,
    pub progress_pct: f32,
    pub total_bytes_processed: u64,
    pub eta_seconds: u64,
    pub stats: BlockStats,
    pub heatmap_blocks: Vec<BlockState>,
    pub probe_result: Option<ProbeTelemetry>,
    pub fix_plan: Option<F3FixPlan>,
    pub error: Option<String>,
}

impl Default for TaskSnapshot {
    fn default() -> Self {
        Self {
            status: TaskStatus::Idle,
            kind: TaskKind::Idle,
            status_message: "就绪".to_string(),
            current_speed_mb: 0.0,
            speed_history: Vec::with_capacity(1200),
            progress_pct: 0.0,
            total_bytes_processed: 0,
            eta_seconds: 0,
            stats: BlockStats::default(),
            heatmap_blocks: vec![BlockState::Untested; 1000],
            probe_result: None,
            fix_plan: None,
            error: None,
        }
    }
}

// 深层引擎控制器 (EngineController)
// 统一管理多线程生命周期、跨线程管道、取消控制与状态平滑采样
pub struct EngineController {
    snapshot: TaskSnapshot,
    cancel_flag: Arc<AtomicBool>,
    h2w_rx: Option<Receiver<H2wTelemetry>>,
    probe_rx: Option<Receiver<ProbeTelemetry>>,
    start_instant: Option<Instant>,
    active_probe_disk_index: Option<u32>,
}

impl Default for EngineController {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineController {
    pub fn new() -> Self {
        Self {
            snapshot: TaskSnapshot::default(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            h2w_rx: None,
            probe_rx: None,
            start_instant: None,
            active_probe_disk_index: None,
        }
    }

    pub fn snapshot(&self) -> &TaskSnapshot {
        &self.snapshot
    }

    pub fn is_running(&self) -> bool {
        self.snapshot.status == TaskStatus::Running
    }

    pub fn cancel(&mut self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        self.snapshot.status_message = "正在请求停止任务...".to_string();
    }

    pub fn set_status_message(&mut self, message: String) {
        self.snapshot.status_message = message;
    }

    // 启动 H2W 测试引擎 (写入、校验或全流程写验)
    pub fn start_h2w(&mut self, config: H2wConfig) {
        self.reset_for_new_task();
        self.snapshot.status = TaskStatus::Running;
        self.snapshot.kind = match config.mode {
            H2wMode::Write => TaskKind::Write,
            H2wMode::Verify => TaskKind::Verify,
            H2wMode::WriteAndVerify => TaskKind::WriteAndVerify,
        };
        self.snapshot.status_message = match config.mode {
            H2wMode::Write => "正在执行无缓冲直通写入...".to_string(),
            H2wMode::Verify => "正在执行流式数据校验...".to_string(),
            H2wMode::WriteAndVerify => "正在执行连续写入与校验...".to_string(),
        };

        self.start_instant = Some(Instant::now());
        self.snapshot.speed_history.push([0.0, 0.0]);

        let (tx, rx) = unbounded();
        self.h2w_rx = Some(rx);

        let flag = self.cancel_flag.clone();
        std::thread::spawn(move || {
            let session = H2wSession::new(config, flag);
            session.run(tx);
        });
    }

    // 启动闪电探针探测，自动完成独占卷卸载与 RAII 锁持有
    pub fn start_probe(&mut self, target: StorageTarget) -> Result<(), String> {
        if !target.device.is_safe_target {
            return Err("所选设备未通过移动总线安全白名单隔离！".to_string());
        }

        let lock_guard = target.prepare_for_probe()?;
        let disk_index = target.device.drive_index;

        self.reset_for_new_task();
        self.snapshot.status = TaskStatus::Running;
        self.snapshot.kind = TaskKind::Probe;
        self.snapshot.status_message = "正在启动跳跃探针探测...".to_string();
        self.active_probe_disk_index = Some(disk_index);

        let (tx, rx) = unbounded();
        self.probe_rx = Some(rx);

        let total_sectors = target.device.total_bytes / 512;
        let config = F3ProbeConfig {
            physical_device_path: target.device.path.clone(),
            total_sectors,
        };

        let flag = self.cancel_flag.clone();
        std::thread::spawn(move || {
            // lock_guard 移交至线程内部全程持有排他锁，线程退出时自动 Drop 释放！
            let _guard = lock_guard;
            let probe = F3Probe::new(config, flag);
            probe.run(tx);
        });

        Ok(())
    }

    // 单帧轮询：吸收全部后台管道消息并更新快照
    pub fn poll(&mut self) -> &TaskSnapshot {
        // 1. 处理 H2W 会话消息
        if let Some(ref rx) = self.h2w_rx {
            while let Ok(msg) = rx.try_recv() {
                self.snapshot.total_bytes_processed = msg.bytes_processed;
                self.snapshot.current_speed_mb = msg.speed_bps / (1024.0 * 1024.0);
                self.snapshot.eta_seconds = msg.eta_seconds;
                self.snapshot.stats = msg.stats;

                if let Some((idx, state_val)) = msg.heatmap_sample {
                    if idx < self.snapshot.heatmap_blocks.len() {
                        self.snapshot.heatmap_blocks[idx] = state_val;
                    }
                }

                if let Some(start) = self.start_instant {
                    let elapsed = start.elapsed().as_secs_f64();
                    self.append_speed(elapsed, self.snapshot.current_speed_mb);
                }

                if msg.phase == H2wPhase::CleaningUp {
                    self.snapshot.status_message = "校验完成，正在自动清理测试文件并释放空间...".to_string();
                }

                if msg.is_finished {
                    self.snapshot.status = if msg.error.is_some() {
                        TaskStatus::Error
                    } else {
                        TaskStatus::Finished
                    };
                    self.snapshot.error = msg.error.clone();
                    self.snapshot.status_message = msg.error.unwrap_or_else(|| {
                        match msg.mode {
                            H2wMode::Write => "写入测试完成".to_string(),
                            H2wMode::Verify => "校验测试完成".to_string(),
                            H2wMode::WriteAndVerify => "全流程写验测试完成".to_string(),
                        }
                    });
                    self.h2w_rx = None;
                    break;
                }
            }
        }

        // 2. 处理探针探测消息
        if let Some(ref rx) = self.probe_rx {
            while let Ok(msg) = rx.try_recv() {
                self.snapshot.progress_pct = msg.progress_pct;
                self.snapshot.status_message = msg.current_phase.clone();

                if msg.is_finished {
                    self.snapshot.status = if msg.error.is_some() {
                        TaskStatus::Error
                    } else {
                        TaskStatus::Finished
                    };
                    self.snapshot.error = msg.error.clone();

                    if msg.is_fake {
                        if let Some(disk_idx) = self.active_probe_disk_index {
                            self.snapshot.fix_plan = Some(F3FixPlan::new(
                                disk_idx,
                                msg.announced_size_bytes,
                                msg.real_size_bytes,
                            ));
                        }
                    }

                    self.snapshot.probe_result = Some(msg);
                    self.probe_rx = None;
                    break;
                }
            }
        }

        &self.snapshot
    }

    fn reset_for_new_task(&mut self) {
        self.cancel_flag.store(false, Ordering::Relaxed);
        self.snapshot.current_speed_mb = 0.0;
        self.snapshot.speed_history.clear();
        self.snapshot.progress_pct = 0.0;
        self.snapshot.total_bytes_processed = 0;
        self.snapshot.eta_seconds = 0;
        self.snapshot.stats = BlockStats::default();
        self.snapshot.heatmap_blocks.fill(BlockState::Untested);
        self.snapshot.error = None;
    }

    fn append_speed(&mut self, time: f64, speed: f64) {
        if self.snapshot.speed_history.len() >= 1200 {
            self.snapshot.speed_history.remove(0);
        }
        self.snapshot.speed_history.push([time, speed]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_controller_defaults() {
        let mut ctrl = EngineController::new();
        let snap = ctrl.poll();
        assert_eq!(snap.status, TaskStatus::Idle);
        assert_eq!(snap.kind, TaskKind::Idle);
        assert_eq!(snap.current_speed_mb, 0.0);
        assert_eq!(snap.heatmap_blocks.len(), 1000);
        assert!(!ctrl.is_running());
    }

    #[test]
    fn test_speed_history_limit() {
        let mut ctrl = EngineController::new();
        for i in 0..1300 {
            ctrl.append_speed(i as f64, 100.0);
        }
        assert_eq!(ctrl.snapshot().speed_history.len(), 1200);
    }
}
