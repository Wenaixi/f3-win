use std::fs::File;
use std::io::Write;
use std::process::Command;
use crate::core::devices::PhysicalDeviceInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixSafetyError {
    SystemDiskForbidden(u32),
    DeviceNotFound(u32),
    NonRemovableBus { disk_index: u32, bus_name: String },
    InvalidPartitionSize { safe_mb: u64, real_bytes: u64 },
    CapacityTooSmall(u64),
    UnsupportedFileSystem(String),
}

impl std::fmt::Display for FixSafetyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SystemDiskForbidden(idx) => write!(f, "安全拦截：禁止在 PhysicalDrive{} (系统主盘) 上执行分区重构", idx),
            Self::DeviceNotFound(idx) => write!(f, "找不到物理磁盘 PhysicalDrive{}，设备可能已断开", idx),
            Self::NonRemovableBus { disk_index, bus_name } => {
                write!(f, "安全硬隔离：PhysicalDrive{} 属于非移动介质总线 ({})，禁止执行破坏性修复", disk_index, bus_name)
            }
            Self::InvalidPartitionSize { safe_mb, real_bytes } => {
                write!(f, "非法分区大小：建议分区 {} MB 超过或未对齐真实物理容量 ({} 字节)", safe_mb, real_bytes)
            }
            Self::CapacityTooSmall(bytes) => write!(f, "实测物理容量过小 ({} 字节)，无法建立安全分区", bytes),
            Self::UnsupportedFileSystem(fs) => write!(f, "不支持的文件系统类型: {}", fs),
        }
    }
}

impl std::error::Error for FixSafetyError {}

// 分区脚本执行器抽象接缝 (DiskpartRunner)
// 遵循深层模块接缝原则：生产环境调用真实 diskpart 进程，单测环境由 Mock 断言脚本内容与错误处理
pub trait DiskpartRunner: Send + Sync {
    fn run_script(&self, script: &str) -> Result<String, String>;
}

// 生产环境真实 Windows diskpart 执行器
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemDiskpartRunner;

impl DiskpartRunner for SystemDiskpartRunner {
    fn run_script(&self, script: &str) -> Result<String, String> {
        let temp_dir = std::env::temp_dir();
        let file_name = format!(
            "f3_fix_{}_{}.diskpart",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let script_path = temp_dir.join(file_name);

        std::fs::write(&script_path, script)
            .map_err(|e| format!("无法创建临时 diskpart 脚本: {}", e))?;

        // RAII 临时文件守卫
        struct TempCleaner<'a>(&'a std::path::Path);
        impl<'a> Drop for TempCleaner<'a> {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0);
            }
        }
        let _cleaner = TempCleaner(&script_path);

        let mut cmd = Command::new("diskpart.exe");
        cmd.arg("/s").arg(&script_path);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW 消除黑框闪烁
        }

        let mut child = cmd.spawn().map_err(|e| {
            format!("启动 diskpart.exe 失败 (请确认具备管理员权限): {}", e)
        })?;

        // 90 秒超时看门狗轮询，杜绝假盘硬件掉线时永久阻塞挂死 Worker 线程
        let timeout = std::time::Duration::from_secs(90);
        let start = std::time::Instant::now();
        let _status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        return Err("diskpart 执行超时 (硬件主控无响应，可能已掉盘或固件死锁)".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => return Err(format!("等待 diskpart 退出失败: {}", e)),
            }
        };

        let output = child.wait_with_output().map_err(|e| format!("获取 diskpart 输出失败: {}", e))?;

        let stdout_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr_str = String::from_utf8_lossy(&output.stderr).trim().to_string();

        let has_err = stdout_str.contains("DiskPart has encountered an error")
            || stdout_str.contains("Virtual Disk Service error")
            || stdout_str.contains("错误")
            || !output.status.success();

        if has_err {
            let detail = if !stderr_str.is_empty() {
                stderr_str
            } else {
                stdout_str
            };
            Err(format!("diskpart 分区修复执行失败: {}", detail))
        } else {
            Ok(stdout_str)
        }
    }