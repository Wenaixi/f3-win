// 核心路径服务与资源管理器交互模块
// 集中管理程序所在目录、工作目录、测试卷目标与维权存证目录

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::Write;

/// 应用程序全局路径信息摘要
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPathInfo {
    /// 可执行程序本身完整路径
    pub exe_path: PathBuf,
    /// 可执行程序所在父级目录
    pub exe_dir: PathBuf,
    /// 进程当前工作目录
    pub working_dir: PathBuf,
    /// 当前用户选中的测试卷目标目录 (若已选择)
    pub selected_volume: Option<PathBuf>,
    /// 维权存证档案目录 (与可执行文件同级目录保持一致)
    pub evidence_dir: PathBuf,
    /// 系统临时文件目录
    pub temp_dir: PathBuf,
}

impl AppPathInfo {
    /// 收集当前运行时所有环境路径信息
    pub fn gather(selected_volume: Option<PathBuf>) -> Self {
        let exe_path = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("f3-win.exe"));
        let exe_dir = exe_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let working_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        let temp_dir = std::env::temp_dir();
        let evidence_dir = exe_dir.clone();

        Self {
            exe_path,
            exe_dir,
            working_dir,
            selected_volume,
            evidence_dir,
            temp_dir,
        }
    }
}

/// 在 Windows 资源管理器中打开目录或精准定位选中文件
pub fn open_in_explorer(path: &Path) -> Result<(), String> {
    if !path.exists() {
        // 若目录尚不存在，尝试向上一级查找
        if let Some(parent) = path.parent() {
            if parent.exists() {
                return Command::new("explorer")
                    .arg(parent)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| format!("启动资源管理器失败: {}", e));
            }
        }
    }

    if path.is_file() {
        // 如果是文件，使用 /select 准确定位高亮目标文件
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path.as_os_str());
        Command::new("explorer")
            .arg(arg)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("定位文件失败: {}", e))
    } else {
        Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开目录失败: {}", e))
    }
}

/// 将指定文本复制到 Windows 系统剪贴板
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut child = Command::new("clip")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("无法启动系统剪贴板进程 (clip.exe): {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }

    let status = child
        .wait()
        .map_err(|e| format!("等待剪贴板操作完成失败: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("clip.exe 返回非零退出码: {:?}", status.code()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_path_info_gather() {
        let selected = Some(PathBuf::from(r"E:\"));
        let info = AppPathInfo::gather(selected.clone());

        assert_eq!(info.selected_volume, selected);
        assert!(!info.exe_dir.as_os_str().is_empty());
        assert!(!info.working_dir.as_os_str().is_empty());
        assert!(!info.temp_dir.as_os_str().is_empty());
        assert_eq!(info.evidence_dir, info.exe_dir);
    }

    #[test]
    fn test_app_path_info_none_selected() {
        let info = AppPathInfo::gather(None);
        assert!(info.selected_volume.is_none());
        assert_eq!(info.evidence_dir, info.exe_dir);
    }
}
