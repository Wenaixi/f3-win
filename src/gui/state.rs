use crate::core::controller::EngineController;
use crate::core::devices::StorageInventory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    WriteTest,
    ReadVerify,
    RapidProbe,
    PartitionFix,
    Settings,
}

// GUI 全局状态容器
// 大幅收敛：原本分散的硬件列表与后台任务管道，统一内聚为 StorageInventory 与 EngineController
pub struct AppState {
    pub current_tab: AppTab,

    // 深层存储资产清单 (管理逻辑卷、物理磁盘与安全移动介质过滤)
    pub inventory: StorageInventory,

    // 深层任务引擎控制器 (管理后台线程、管道、取消标志与状态采样)
    pub controller: EngineController,

    // 用户测试配置与高级设置
    pub salt: u64,
    pub use_random_salt: bool,
    pub chunk_size_mb: usize,
    pub start_file_num: u64,
    pub end_file_num: Option<u64>,
    pub max_test_gb: Option<u64>,
    pub auto_cleanup_after_read: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            current_tab: AppTab::WriteTest,
            inventory: StorageInventory::new(),
            controller: EngineController::new(),
            salt: 0,
            use_random_salt: true, // 默认开启单向安全加盐，开箱即防算力回放作弊
            chunk_size_mb: 1,
            start_file_num: 0,
            end_file_num: None,
            max_test_gb: None,
            auto_cleanup_after_read: false,
        }
    }
}
