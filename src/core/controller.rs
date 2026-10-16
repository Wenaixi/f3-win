// 扇区状态严重度权重 (Bad > Overwritten > Changed > Good > Untested)
#[inline]
fn state_severity(state: BlockState) -> u8 {
    match state {
        BlockState::Bad => 4,
        BlockState::Overwritten => 3,
        BlockState::Changed => 2,
        BlockState::Good => 1,
        BlockState::Untested => 0,
    }
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use crossbeam_channel::{unbounded, Receiver};

use crate::core::algo::{BlockState, BlockStats};
use crate::core::devices::StorageTarget;
use crate::core::fix::{F3FixPlan, SafePartitionFixer};
use crate::core::report::DiagnosticReport;
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
    Fix,     // 新增：安全分区重构
    Cleanup, // 新增：测试文件碎片清理
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
    pub report: Option<DiagnosticReport>,
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
            report: None,
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
    fix_rx: Option<Receiver<Result<String, String>>>,
    cleanup_rx: Option<Receiver<Result<(usize, u64), String>>>,
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
            fix_rx: None,
            cleanup_rx: None,
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
        self.cancel_flag.store(true, Ordering::Release);
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

    // 启动闪电探针探测，通过 StorageTarget 统一交付 BlockIo 接缝与 RAII 锁守卫
    pub fn start_probe(&mut self, target: StorageTarget) -> Result<(), String> {
        if !target.device.is_safe_target {
            return Err("所选设备未通过移动总线安全白名单隔离！".to_string());
        }

        // 一站式完成卷独占锁定并打开物理块设备
        let (mut block_file, lock_guard) = target.open_block_io()?;
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
            total_sectors,
        };

        let flag = self.cancel_flag.clone();
        std::thread::spawn(move || {
            // lock_guard 与 block_file 移交至线程内部全程持有，线程退出时自动 Drop 释放！
            let _guard = lock_guard;
            let probe = F3Probe::new(config, flag);
            let _ = probe.run(&mut block_file, tx);
        });

        Ok(())
    }

    // 启动安全分区自愈修复 (后台异步拉起 diskpart)
    pub fn start_fix(&mut self, plan: F3FixPlan) -> Result<(), String> {
        let fixer = SafePartitionFixer::new(plan);
        // 先行同步校验安全白名单，杜绝主盘与非法总线误操作
        fixer.verify_safety().map_err(|e| format!("安全拦截: {}", e))?;

        self.reset_for_new_task();
        self.snapshot.status = TaskStatus::Running;
        self.snapshot.kind = TaskKind::Fix;
        self.snapshot.status_message = "正在执行安全分区重构 (运行 diskpart)...".to_string();

        let (tx, rx) = unbounded();
        self.fix_rx = Some(rx);

        std::thread::spawn(move || {
            let res = fixer.apply();
            let _ = tx.send(res);
        });

        Ok(())
    }

    // 启动后台异步 .h2w 碎片清理
    pub fn start_cleanup(&mut self, target_path: PathBuf) {
        self.reset_for_new_task();
        self.snapshot.status = TaskStatus::Running;
        self.snapshot.kind = TaskKind::Cleanup;
        self.snapshot.status_message = "正在扫描并清理 .h2w 测试碎片...".to_string();

        let (tx, rx) = unbounded();
        self.cleanup_rx = Some(rx);

        std::thread::spawn(move || {
            let res = H2wSession::cleanup_h2w_files(&target_path);
            let _ = tx.send(res);
        });
    }

    // 单帧轮询：吸收全部后台管道消息并更新快照
    pub fn poll(&mut self) -> &TaskSnapshot {
        // 1. 处理 H2W 会话消息
        if let Some(rx) = self.h2w_rx.clone() {
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        self.snapshot.total_bytes_processed = msg.bytes_processed;
                        self.snapshot.current_speed_mb = msg.speed_bps / (1024.0 * 1024.0);
                        self.snapshot.eta_seconds = msg.eta_seconds;
                        self.snapshot.stats = msg.stats;

                        if let Some((idx, state_val)) = msg.heatmap_sample {
                            if idx < self.snapshot.heatmap_blocks.len() {
                                // 严重度单调递增原则：低严重度状态不得覆盖已有高严重度异常状态
                                let current_sev = state_severity(self.snapshot.heatmap_blocks[idx]);
                                let new_sev = state_severity(state_val);
                                if new_sev >= current_sev {
                                    self.snapshot.heatmap_blocks[idx] = state_val;
                                }
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
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        self.snapshot.status = TaskStatus::Error;
                        self.snapshot.status_message = "后台工作线程异常终止 (管道已断开)".to_string();
                        self.snapshot.error = Some("后台任务非正常退出".to_string());
                        self.h2w_rx = None;
                        break;
                    }
                }
            }
        }

        // 2. 处理探针探测消息
        if let Some(rx) = self.probe_rx.clone() {
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

        // 3. 处理分区修复异步结果
        if let Some(rx) = self.fix_rx.clone() {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok(msg) => {
                        self.snapshot.status = TaskStatus::Finished;
                        self.snapshot.status_message = format!("分区修复成功: {}", msg);
                    }
                    Err(e) => {
                        self.snapshot.status = TaskStatus::Error;
                        self.snapshot.error = Some(e.clone());
                        self.snapshot.status_message = format!("分区修复失败: {}", e);
                    }
                }
                self.fix_rx = None;
            }
        }

        // 4. 处理后台清理异步结果
        if let Some(rx) = self.cleanup_rx.clone() {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok((cnt, bytes)) => {
                        self.snapshot.status = TaskStatus::Finished;
                        self.snapshot.status_message = format!(
                            "已成功清理 {} 个文件，释放 {:.2} GB 磁盘空间！",
                            cnt,
                            bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        );
                    }
                    Err(e) => {
                        self.snapshot.status = TaskStatus::Error;
                        self.snapshot.error = Some(e.clone());
                        self.snapshot.status_message = format!("清理失败: {}", e);
                    }
                }
                self.cleanup_rx = None;
            }
        }

        &self.snapshot
    }

    fn reset_for_new_task(&mut self) {
        // 关键修复 1: 隔离全新原子标志，杜绝串扰未完全退出的旧 Worker 线程
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        self.snapshot.current_speed_mb = 0.0;
        self.snapshot.speed_history.clear();
        self.snapshot.progress_pct = 0.0;
        self.snapshot.total_bytes_processed = 0;
        self.snapshot.eta_seconds = 0;
        self.snapshot.stats = BlockStats::default();
        self.snapshot.heatmap_blocks.fill(BlockState::Untested);
        self.snapshot.error = None;

        // 关键修复 2: 彻底清空上一任务遗留的幽灵探测结果与修复计划，防止误格正常盘
        self.snapshot.probe_result = None;
        self.snapshot.fix_plan = None;
        self.snapshot.report = None;
        self.active_probe_disk_index = None;
        self.start_instant = None;

        // 关键修复 3: 主动断开残余旧管道句柄
        self.h2w_rx = None;
        self.probe_rx = None;
        self.fix_rx = None;
        self.cleanup_rx = None;
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

    #[test]
    fn test_start_fix_rejects_disk_zero() {
        let mut ctrl = EngineController::new();
        let plan = F3FixPlan::new(0, 128 * 1024 * 1024 * 1024, 32 * 1024 * 1024 * 1024);
        let res = ctrl.start_fix(plan);
        assert!(res.is_err(), "控制器必须在同步阶段立即拦截主盘修复");
        let err_msg = res.unwrap_err();
        assert!(err_msg.contains("安全拦截"));
        assert_eq!(ctrl.snapshot().status, TaskStatus::Idle);
    }

    #[test]
    fn test_start_cleanup_transitions_to_running() {
        let mut ctrl = EngineController::new();
        ctrl.start_cleanup(std::path::PathBuf::from("Z:\\fake_nonexistent_path"));
        assert_eq!(ctrl.snapshot().status, TaskStatus::Running);
        assert_eq!(ctrl.snapshot().kind, TaskKind::Cleanup);
    }

    #[test]
    fn test_start_probe_rejects_unsafe_target() {
        let mut ctrl = EngineController::new();
        let dev = crate::core::devices::PhysicalDeviceInfo {
            drive_index: 0,
            path: r"\\.\PhysicalDrive0".to_string(),
            vendor_id: "Internal".to_string(),
            product_id: "Disk".to_string(),
            serial_number: "000".to_string(),
            bus_type: crate::core::devices::StorageBusType::Nvme,
            total_bytes: 1000,
            is_removable: false,
            is_safe_target: false,
            warning: None,
            mounted_volumes: Vec::new(),
        };
        let target = StorageTarget::new(dev, Vec::new());
        let res = ctrl.start_probe(target);
        assert!(res.is_err(), "内部固定磁盘必须被拒绝对其执行物理探测");
        assert!(res.unwrap_err().contains("安全白名单"));
    }

    #[test]
    fn test_controller_cancel_sets_flag() {
        let mut ctrl = EngineController::new();
        ctrl.cancel();
        assert!(ctrl.cancel_flag.load(Ordering::Acquire), "cancel() 必须置位 cancel_flag");
        assert!(ctrl.snapshot().status_message.contains("停止"));
    }
    #[test]
    fn test_state_severity_ordering() {
        assert!(state_severity(BlockState::Bad) > state_severity(BlockState::Overwritten));
        assert!(state_severity(BlockState::Overwritten) > state_severity(BlockState::Changed));
        assert!(state_severity(BlockState::Changed) > state_severity(BlockState::Good));
        assert!(state_severity(BlockState::Good) > state_severity(BlockState::Untested));
    }

    #[test]
    fn test_channel_disconnect_recovery() {
        let mut ctrl = EngineController::new();
        let (tx, rx) = crossbeam_channel::unbounded::<H2wTelemetry>();
        ctrl.h2w_rx = Some(rx);
        ctrl.snapshot.status = TaskStatus::Running;

        // 显式丢弃发送端，制造管道异常断开
        drop(tx);

        let snap = ctrl.poll();
        assert_eq!(snap.status, TaskStatus::Error, "通道断开必须自愈转换为 Error 状态");
        assert!(snap.status_message.contains("管道已断开"));
        assert!(ctrl.h2w_rx.is_none(), "断开后必须释放接收端句柄");
    }
}