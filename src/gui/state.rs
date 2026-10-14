use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Receiver;

use crate::core::algo::{BlockState, BlockStats};
use crate::core::devices::PhysicalDeviceInfo;
use crate::core::fix::F3FixPlan;
use crate::core::probe::ProbeTelemetry;
use crate::core::reader::ReadTelemetry;
use crate::core::writer::WriteTelemetry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    WriteTest,
    ReadVerify,
    RapidProbe,
    PartitionFix,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Idle,
    Running,
    Finished,
    Error,
}

pub struct AppState {
    pub current_tab: AppTab,
    pub status: TaskStatus,
    pub status_message: String,

    // 选中的设备与路径
    pub selected_path: PathBuf,
    pub selected_device: Option<PhysicalDeviceInfo>,
    pub devices: Vec<PhysicalDeviceInfo>,

    // 高级设置
    pub salt: u64,
    pub use_random_salt: bool,
    pub chunk_size_mb: usize,

    // 运行指标遥测
    pub speed_history: Vec<[f64; 2]>, // [时间戳(秒), 速度(MB/s)]
    pub current_speed_mb: f64,
    pub total_bytes_processed: u64,
    pub eta_seconds: u64,
    pub progress_pct: f32,

    // 扇区健康热力块网格 (1000 个采样块)
    pub heatmap_blocks: Vec<BlockState>,
    pub stats: BlockStats,

    // 探测与修复结果
    pub probe_result: Option<ProbeTelemetry>,
    pub fix_plan: Option<F3FixPlan>,

    // 任务控制
    pub cancel_flag: Arc<AtomicBool>,
    pub write_rx: Option<Receiver<WriteTelemetry>>,
    pub read_rx: Option<Receiver<ReadTelemetry>>,
    pub probe_rx: Option<Receiver<ProbeTelemetry>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            current_tab: AppTab::WriteTest,
            status: TaskStatus::Idle,
            status_message: "就绪".to_string(),
            selected_path: PathBuf::from("E:\\"),
            selected_device: None,
            devices: Vec::new(),
            salt: 0,
            use_random_salt: false,
            chunk_size_mb: 1,
            speed_history: Vec::new(),
            current_speed_mb: 0.0,
            total_bytes_processed: 0,
            eta_seconds: 0,
            progress_pct: 0.0,
            heatmap_blocks: vec![BlockState::Good; 1000],
            stats: BlockStats::default(),
            probe_result: None,
            fix_plan: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            write_rx: None,
            read_rx: None,
            probe_rx: None,
        }
    }
}

impl AppState {
    pub fn reset_telemetry(&mut self) {
        self.speed_history.clear();
        self.current_speed_mb = 0.0;
        self.total_bytes_processed = 0;
        self.eta_seconds = 0;
        self.progress_pct = 0.0;
        self.heatmap_blocks = vec![BlockState::Good; 1000];
        self.stats = BlockStats::default();
        self.cancel_flag = Arc::new(AtomicBool::new(false));
    }
}
