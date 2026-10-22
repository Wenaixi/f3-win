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
    pub salt_hex: String,
    pub chunk_size_mb: usize,
    pub start_file_num: u64,
    pub end_file_num: Option<u64>,
    pub max_test_gb: Option<u64>,
    pub auto_cleanup_after_read: bool,

    // 全局持久化的读写速度全景与生理审计档案 (写入与读取双端并存)
    pub audit_record: SpeedAuditRecord,

    // 记录上一次选中的测试卷路径 (用于盘符切换时原子重置审计报告，杜绝跨盘幽灵报告)
    pub last_selected_path: Option<std::path::PathBuf>,

    // 物理分区自愈现场安全复核缓存 (避免 60FPS UI 循环中每 16ms 轮询底层 Win32 IOCTL)
    pub cached_dry_run_report: Option<(u32, Result<crate::core::fix::DryRunReport, String>)>,

    // 系统托盘与弹窗控制状态
    pub minimize_to_tray_on_close: bool,
    pub show_path_info_modal: bool,
    pub show_exit_confirm_modal: bool,

    // 闪电探测前用户是否已勾选确认备份数据且知悉后续需格式化
    pub probe_backup_confirmed: bool,
}

/// 读写双向速度与生理特征审计记录
#[derive(Debug, Clone, Default)]
pub struct SpeedAuditRecord {
    pub write_tested: bool,
    pub write_avg_mb: f64,
    pub write_peak_mb: f64,
    pub write_valley_mb: f64,
    pub write_median_mb: f64,
    pub write_bytes: u64,
    pub write_duration_secs: f64,
    pub slc_cliff_bytes: Option<u64>,
    pub slc_cliff_speed_before_mb: f64,
    pub slc_cliff_speed_after_mb: f64,
    pub slc_cliff_drop_pct: f64,

    pub read_tested: bool,
    pub read_avg_mb: f64,
    pub read_bytes: u64,
    pub read_duration_secs: f64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            current_tab: AppTab::WriteTest,
            inventory: StorageInventory::new(),
            controller: EngineController::new(),
            salt: 0,
            use_random_salt: true, // 默认开启高安全防伪加盐模式，防固件模逆元算力回放作弊；可自主取消以兼容原版
            salt_hex: String::new(),
            chunk_size_mb: 1,
            start_file_num: 0,
            end_file_num: None,
            max_test_gb: None,
            auto_cleanup_after_read: false,
            audit_record: SpeedAuditRecord::default(),
            last_selected_path: None,
            cached_dry_run_report: None,
            minimize_to_tray_on_close: false,
            show_path_info_modal: false,
            show_exit_confirm_modal: false,
            probe_backup_confirmed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_backup_confirmed_default_false() {
        let state = AppState::default();
        assert!(!state.probe_backup_confirmed, "为确保用户数据安全，闪电探测备份确认开关默认必须为未勾选状态");
    }

    #[test]
    fn test_minimize_to_tray_on_close_default_false() {
        let state = AppState::default();
        assert!(!state.minimize_to_tray_on_close, "默认点击关闭按钮必须直接安全退出并销毁托盘，绝不偷偷驻留后台");
    }
}