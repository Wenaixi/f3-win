use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use crossbeam_channel::Receiver;

use crate::core::algo::{BlockState, BlockStats};
use crate::core::devices::{PhysicalDeviceInfo, VolumeInfo};
use crate::core::fix::F3FixPlan;
use crate::core::probe::ProbeTelemetry;
use crate::core::session::H2wTelemetry;

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
    pub volumes: Vec<VolumeInfo>,
    pub selected_volume_index: usize,

    // 高级设置
    pub salt: u64,
    pub use_random_salt: bool,
    pub chunk_size_mb: usize,
    pub start_file_num: u64,
    pub end_file_num: Option<u64>,
    pub max_test_gb: Option<u64>,
    pub auto_cleanup_after_read: bool,

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

    // 任务控制 (统一的 H2W 测试引擎通道与探针通道)
    pub cancel_flag: Arc<AtomicBool>,
    pub h2w_rx: Option<Receiver<H2wTelemetry>>,
    pub probe_rx: Option<Receiver<ProbeTelemetry>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            current_tab: AppTab::WriteTest,
            status: TaskStatus::Idle,
            status_message: "就绪".to_string(),
            selected_path: PathBuf::from(r"E:\"),
            selected_device: None,
            devices: Vec::new(),
            volumes: Vec::new(),
            selected_volume_index: 0,
            salt: 0,
            use_random_salt: false,
            chunk_size_mb: 1,
            start_file_num: 0,
            end_file_num: None,
            max_test_gb: None,
            auto_cleanup_after_read: false,
            speed_history: Vec::new(),
            current_speed_mb: 0.0,
            total_bytes_processed: 0,
            eta_seconds: 0,
            progress_pct: 0.0,
            heatmap_blocks: vec![BlockState::Untested; 1000],
            stats: BlockStats::default(),
            probe_result: None,
            fix_plan: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            h2w_rx: None,
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
        self.heatmap_blocks = vec![BlockState::Untested; 1000];
        self.stats = BlockStats::default();
        self.cancel_flag = Arc::new(AtomicBool::new(false));
    }
}
