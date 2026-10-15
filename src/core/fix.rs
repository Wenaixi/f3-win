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

#[derive(Debug, Clone)]
pub struct DryRunReport {
    pub disk_index: u32,
    pub vendor_id: String,
    pub product_id: String,
    pub bus_name: String,
    pub safe_partition_mb: u64,
    pub file_system: String,
    pub script_content: String,
}

#[derive(Debug, Clone)]
pub struct F3FixPlan {
    pub disk_index: u32,
    pub announced_bytes: u64,
    pub real_bytes: u64,
    pub safe_partition_bytes: u64,
    pub safe_partition_mb: u64,
    pub file_system: String, // "exfat", "fat32", "ntfs"
}

impl F3FixPlan {
    // 从实测容量计算安全修复方案
    pub fn new(disk_index: u32, announced_bytes: u64, real_bytes: u64) -> Self {
        // 安全策略：如果实测容量大于 64MB，预留 16MB 缓冲以防主控极端元数据越界，并对齐到 1MB
        let safety_margin = 16 * 1024 * 1024u64;
        let usable = if real_bytes > safety_margin {
            real_bytes - safety_margin
        } else {
            real_bytes
        };

        // 截断至 1MB 整数倍
        let safe_partition_bytes = usable - (usable % (1024 * 1024));
        let safe_partition_mb = safe_partition_bytes / (1024 * 1024);

        let file_system = if safe_partition_mb > 32 * 1024 {
            "exfat".to_string()
        } else {
            "fat32".to_string()
        };

        Self {
            disk_index,
            announced_bytes,
            real_bytes,
            safe_partition_bytes,
            safe_partition_mb,
            file_system,
        }
    }

    // 生成标准的 Windows diskpart 脚本内容
    pub fn generate_diskpart_script(&self) -> String {
        format!(
            "# F3-Win 自动生成的安全分区修复脚本\n\
            # 原始标称容量: {} 字节 ({:.2} GB)\n\
            # 真实物理容量: {} 字节 ({:.2} GB)\n\
            # 安全截断分区: {} MB\n\
            select disk {}\n\
            clean\n\
            create partition primary size={}\n\
            format fs={} quick\n\
            assign\n\
            active\n",
            self.announced_bytes,
            self.announced_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            self.real_bytes,
            self.real_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            self.safe_partition_mb,
            self.disk_index,
            self.safe_partition_mb,
            self.file_system,
        )
    }

    // 将脚本保存到指定文件
    pub fn export_script_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        let script = self.generate_diskpart_script();
        let mut file = File::create(path).map_err(|e| format!("无法创建脚本文件: {}", e))?;
        file.write_all(script.as_bytes())
            .map_err(|e| format!("写入脚本文件失败: {}", e))?;
        Ok(())
    }

    // 快捷执行 (直接委托给 SafePartitionFixer)
    pub fn execute_auto_fix(&self) -> Result<String, String> {
        let fixer = SafePartitionFixer::new(self.clone());
        fixer.execute()
    }
}

// 自守卫分区自愈事务执行器 (SafePartitionFixer)
// 包含物理磁盘即时总线复核、系统主盘绝缘隔离、Dry-Run 预演与 RAII 临时文件守卫
#[derive(Debug, Clone)]
pub struct SafePartitionFixer {
    pub plan: F3FixPlan,
}

impl SafePartitionFixer {
    pub fn new(plan: F3FixPlan) -> Self {
        Self { plan }
    }

    // 执行完整安全校验：拦截系统主盘、校验容量边界并现场复核总线属性
    pub fn verify_safety(&self) -> Result<PhysicalDeviceInfo, FixSafetyError> {
        // 1. 绝对防御：严禁触碰 PhysicalDrive0
        if self.plan.disk_index == 0 {
            return Err(FixSafetyError::SystemDiskForbidden(0));
        }

        // 2. 容量参数有效性校验
        if self.plan.real_bytes < 16 * 1024 * 1024 {
            return Err(FixSafetyError::CapacityTooSmall(self.plan.real_bytes));
        }
        if self.plan.safe_partition_mb == 0 || self.plan.safe_partition_bytes > self.plan.real_bytes {
            return Err(FixSafetyError::InvalidPartitionSize {
                safe_mb: self.plan.safe_partition_mb,
                real_bytes: self.plan.real_bytes,
            });
        }
        let valid_fs = ["fat32", "exfat", "ntfs"];
        if !valid_fs.contains(&self.plan.file_system.to_ascii_lowercase().as_str()) {
            return Err(FixSafetyError::UnsupportedFileSystem(self.plan.file_system.clone()));
        }

        // 3. 硬件现场总线即时复核 (消除 TOCTOU 插拔时间差)
        let dev = PhysicalDeviceInfo::query_by_index(self.plan.disk_index)
            .ok_or(FixSafetyError::DeviceNotFound(self.plan.disk_index))?;

        if !dev.bus_type.is_removable_bus() && !dev.is_removable {
            return Err(FixSafetyError::NonRemovableBus {
                disk_index: self.plan.disk_index,
                bus_name: dev.bus_type.display_name().to_string(),
            });
        }

        Ok(dev)
    }

    // 预演执行：生成脚本与核验报告，零磁盘写操作
    pub fn dry_run(&self) -> Result<DryRunReport, FixSafetyError> {
        let dev = self.verify_safety()?;
        let script = self.plan.generate_diskpart_script();
        Ok(DryRunReport {
            disk_index: self.plan.disk_index,
            vendor_id: dev.vendor_id,
            product_id: dev.product_id,
            bus_name: dev.bus_type.display_name().to_string(),
            safe_partition_mb: self.plan.safe_partition_mb,
            file_system: self.plan.file_system.clone(),
            script_content: script,
        })
    }

    // 安全执行 Windows diskpart 自动化修复
    pub fn execute(&self) -> Result<String, String> {
        let _report = self.dry_run().map_err(|e| format!("安全拦截: {}", e))?;

        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join(format!("f3_safe_fix_disk_{}.diskpart", self.plan.disk_index));
        self.plan.export_script_to_file(&script_path)?;

        // RAII 清理守卫，确保临时脚本在函数返回时 100% 自动被清理
        struct TempScriptCleaner<'a>(&'a std::path::Path);
        impl<'a> Drop for TempScriptCleaner<'a> {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0);
            }
        }
        let _cleaner = TempScriptCleaner(&script_path);

        let mut cmd = Command::new("diskpart.exe");
        cmd.arg("/s").arg(&script_path);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let output = cmd.output().map_err(|e| {
            format!("启动 diskpart.exe 失败 (请确认具备管理员权限): {}", e)
        })?;

        let stdout_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr_str = String::from_utf8_lossy(&output.stderr).trim().to_string();

        let has_diskpart_err = stdout_str.contains("DiskPart has encountered an error")
            || stdout_str.contains("Virtual Disk Service error")
            || stdout_str.contains("错误")
            || !output.status.success();

        if has_diskpart_err {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_zero_rejected() {
        let plan = F3FixPlan::new(0, 128 * 1024 * 1024 * 1024, 32 * 1024 * 1024 * 1024);
        let fixer = SafePartitionFixer::new(plan);
        match fixer.verify_safety() {
            Err(FixSafetyError::SystemDiskForbidden(0)) => (),
            other => panic!("预期拦截磁盘 0，实际得到: {:?}", other),
        }
    }

    #[test]
    fn test_capacity_underflow_rejected() {
        let plan = F3FixPlan::new(1, 64 * 1024 * 1024 * 1024, 8 * 1024 * 1024);
        let fixer = SafePartitionFixer::new(plan);
        assert!(matches!(fixer.verify_safety(), Err(FixSafetyError::CapacityTooSmall(_))));
    }
}
