use std::fs::File;
use std::io::Write;
use std::process::Command;

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

    // 调用 Windows diskpart.exe 执行一键全自动安全修复
    pub fn execute_auto_fix(&self) -> Result<String, String> {
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join(format!("f3_fix_disk_{}.diskpart", self.disk_index));

        // 写入临时脚本
        self.export_script_to_file(&script_path)?;

        // 执行 diskpart /s <script_path>
        let output = Command::new("diskpart.exe")
            .arg("/s")
            .arg(&script_path)
            .output()
            .map_err(|e| format!("执行 diskpart 失败 (请确认是否以管理员身份运行): {}", e))?;

        // 清理临时文件
        let _ = std::fs::remove_file(&script_path);

        if output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(stdout_str)
        } else {
            let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
            Err(format!("diskpart 执行报错退出: {}", stderr_str))
        }
    }
}
