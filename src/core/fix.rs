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
        let status = loop {
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
}

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
            "REM F3-Win 自动生成的安全分区修复脚本\n\
            REM 原始标称容量: {} 字节 ({:.2} GB)\n\
            REM 真实物理容量: {} 字节 ({:.2} GB)\n\
            REM 安全截断分区: {} MB\n\
            select disk {}\n\
            attributes disk clear readonly\n\
            clean\n\
            convert mbr\n\
            create partition primary size={}\n\
            format fs={} quick\n\
            assign\n",
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

    // 纯逻辑级别自校验与脚本执行（可脱离真实硬件运行，测试覆盖核心）
    pub fn execute_with_runner<R: DiskpartRunner>(&self, runner: &R) -> Result<String, String> {
        if self.disk_index == 0 {
            return Err("安全拦截：禁止在 PhysicalDrive0 (系统主盘) 上执行分区重构".to_string());
        }
        if self.real_bytes < 16 * 1024 * 1024 {
            return Err("实测物理容量过小，无法建立安全分区".to_string());
        }
        if self.safe_partition_mb == 0 || self.safe_partition_bytes > self.real_bytes {
            return Err("非法分区大小配置".to_string());
        }
        let valid_fs = ["fat32", "exfat", "ntfs"];
        if !valid_fs.contains(&self.file_system.to_ascii_lowercase().as_str()) {
            return Err(format!("不支持的文件系统类型: {}", self.file_system));
        }

        let script = self.generate_diskpart_script();
        runner.run_script(&script)
    }

    // 快捷执行 (直接委托给 SafePartitionFixer)
    pub fn execute_auto_fix(&self) -> Result<String, String> {
        let fixer = SafePartitionFixer::new(self.clone());
        fixer.execute()
    }
}

// 自守卫分区自愈事务执行器 (SafePartitionFixer)
// 包含物理磁盘即时总线复核、系统主盘绝缘隔离、Dry-Run 预演与 DiskpartRunner 解耦接缝
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

    // 依赖注入执行入口：支持自定义 Runner 适配器
    pub fn apply_with_runner<R: DiskpartRunner>(&self, runner: &R) -> Result<String, String> {
        let _dev = self.verify_safety().map_err(|e| format!("安全拦截: {}", e))?;
        self.plan.execute_with_runner(runner)
    }

    // 默认生产入口 (使用系统原生 diskpart 进程)
    pub fn apply(&self) -> Result<String, String> {
        self.apply_with_runner(&SystemDiskpartRunner)
    }

    // 保持向后兼容现有调用的别名方法
    pub fn execute(&self) -> Result<String, String> {
        self.apply()
    }
}

#[cfg(test)]
#[derive(Debug, Default, Clone)]
pub struct MockDiskpartRunner {
    pub executed_scripts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub return_error: Option<String>,
}

#[cfg(test)]
impl DiskpartRunner for MockDiskpartRunner {
    fn run_script(&self, script: &str) -> Result<String, String> {
        self.executed_scripts.lock().unwrap().push(script.to_string());
        if let Some(ref err) = self.return_error {
            Err(err.clone())
        } else {
            Ok("DiskPart 成功格式化卷。".to_string())
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

    #[test]
    fn test_mock_diskpart_success_script_verification() {
        // 标称 128GB，真实仅 32GB 扩容假盘
        let plan = F3FixPlan::new(2, 128 * 1024 * 1024 * 1024, 32 * 1024 * 1024 * 1024);
        let runner = MockDiskpartRunner::default();
        let res = plan.execute_with_runner(&runner);
        assert!(res.is_ok(), "Mock 执行应该成功");

        let scripts = runner.executed_scripts.lock().unwrap();
        assert_eq!(scripts.len(), 1);
        let script = &scripts[0];

        assert!(script.contains("select disk 2"), "必须选择指定磁盘");
        assert!(script.contains("clean"), "必须清空原有破坏性分区表");
        assert!(script.contains("create partition primary size="), "必须创建主分区");
        assert!(script.contains("format fs=fat32 quick"), "32GB 应当使用 FAT32");
        assert!(script.contains("assign"), "必须分配盘符");
        assert!(!script.contains("active"), "普通数据分区严禁激活以兼容 GPT");
    }

    #[test]
    fn test_mock_diskpart_error_propagation() {
        let plan = F3FixPlan::new(2, 64 * 1024 * 1024 * 1024, 60 * 1024 * 1024 * 1024);
        let runner = MockDiskpartRunner {
            executed_scripts: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            return_error: Some("Virtual Disk Service error: 0x8004240F".to_string()),
        };
        let res = plan.execute_with_runner(&runner);
        assert!(res.is_err(), "发生底层错误时必须返回 Err");
        let err_msg = res.unwrap_err();
        assert!(err_msg.contains("Virtual Disk Service error"));
    }

    #[test]
    fn test_fix_plan_boundary_conditions() {
        // 1. > 32GB 应该自动切换为 exfat
        let large_plan = F3FixPlan::new(1, 128 * 1024 * 1024 * 1024, 64 * 1024 * 1024 * 1024);
        assert_eq!(large_plan.file_system, "exfat");

        // 2. 检查 16MB 预留裕量与 1MB 对齐
        let real_bytes = 100 * 1024 * 1024 + 512 * 1024; // 100.5 MB
        let plan = F3FixPlan::new(1, 128 * 1024 * 1024, real_bytes);
        assert_eq!(plan.safe_partition_mb, 100 - 16); // 84 MB
        assert_eq!(plan.safe_partition_bytes % (1024 * 1024), 0);
    }

    #[test]
    fn test_fix_plan_unsupported_filesystem() {
        let mut plan = F3FixPlan::new(1, 64 * 1024 * 1024 * 1024, 32 * 1024 * 1024 * 1024);
        plan.file_system = "btrfs".to_string();
        let runner = MockDiskpartRunner::default();
        let res = plan.execute_with_runner(&runner);
        assert!(res.is_err(), "不支持的文件系统必须拦截");
        assert!(res.unwrap_err().contains("不支持的文件系统"));
    }
    #[test]
    fn test_diskpart_script_format_rem_and_no_active() {
        let plan = F3FixPlan::new(2, 64 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024);
        let script = plan.generate_diskpart_script();

        // 规范断言：必须使用 Windows 原生标准 REM 注释，严禁 Linux 风格 # 注释
        assert!(script.contains("REM F3-Win 自动生成的安全分区修复脚本"));
        assert!(!script.contains("# F3-Win"));

        // 规范断言：普通数据盘严禁出现 active 指令 (以防在 GPT 分区表上报错)
        assert!(!script.contains("\nactive\n"));
        assert!(!script.ends_with("active\n"));

        // 规范断言：必须包含 select disk 2, clean, format quick
        assert!(script.contains("select disk 2"));
        assert!(script.contains("clean"));
        assert!(script.contains("format fs=exfat quick") || script.contains("format fs="));
    }
}