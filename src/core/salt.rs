use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

/// 单个驱动器的加盐防伪记录
#[derive(Debug, Clone)]
pub struct SaltRecord {
    pub drive_path: String,
    pub volume_serial: Option<u32>,
    pub salt: u64,
    pub salt_hex: String,
    pub updated_at: u64,
}

/// 现代防伪凭证注册表 (存储在 exe 同级目录下，纯标准库无外部依赖)
#[derive(Debug, Clone, Default)]
pub struct SaltRegistry {
    pub records: HashMap<String, SaltRecord>,
}

impl SaltRegistry {
    const REGISTRY_FILENAME: &'static str = "f3_salt_registry.txt";

    /// 获取 exe 所在目录下的注册表文件完整路径
    pub fn get_registry_path() -> PathBuf {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join(Self::REGISTRY_FILENAME)
    }

    /// 从 exe 同级目录加载注册表
    pub fn load() -> Self {
        let path = Self::get_registry_path();
        let mut registry = Self {
            records: HashMap::new(),
        };

        if !path.exists() {
            return registry;
        }

        if let Ok(mut f) = File::open(&path) {
            let mut content = String::new();
            if f.read_to_string(&mut content).is_ok() {
                for line in content.lines() {
                    let parts: Vec<&str> = line.trim().split('|').collect();
                    if parts.len() >= 5 {
                        let key = parts[0].to_string();
                        let drive_path = parts[1].to_string();
                        let serial = parts[2].parse::<u32>().ok();
                        let salt = parts[3].parse::<u64>().unwrap_or(0);
                        let salt_hex = parts[4].to_string();
                        let updated_at = parts.get(5).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

                        registry.records.insert(
                            key,
                            SaltRecord {
                                drive_path,
                                volume_serial: serial,
                                salt,
                                salt_hex,
                                updated_at,
                            },
                        );
                    }
                }
            }
        }

        registry
    }

    /// 将注册表原子持久化落盘至 exe 同级目录下
    pub fn save(&self) -> Result<(), String> {
        let path = Self::get_registry_path();
        let mut lines = Vec::new();

        for (k, r) in &self.records {
            let serial_str = r.volume_serial.map(|s| s.to_string()).unwrap_or_default();
            lines.push(format!(
                "{}|{}|{}|{}|{}|{}",
                k, r.drive_path, serial_str, r.salt, r.salt_hex, r.updated_at
            ));
        }

        let content = lines.join("\n");
        let tmp_path = path.with_extension("tmp");
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| format!("打开 Salt 注册表临时文件失败 ({:?}): {}", tmp_path, e))?;

            file.write_all(content.as_bytes())
                .map_err(|e| format!("写入 Salt 注册表临时文件失败: {}", e))?;
            file.flush()
                .map_err(|e| format!("冲刷 Salt 注册表临时文件失败: {}", e))?;
        }

        std::fs::rename(&tmp_path, &path)
            .map_err(|e| format!("原子更新 Salt 注册表失败 ({:?} -> {:?}): {}", tmp_path, path, e))?;

        Ok(())
    }

    /// 提取指定盘符路径的卷序列号 (如 "G:\\")
    pub fn get_volume_serial(drive_path: &str) -> Option<u32> {
        // 严格标准化为 Windows 卷根路径 (例如 "H:\")
        let first_char = drive_path.chars().find(|c| c.is_ascii_alphabetic())?.to_ascii_uppercase();
        let clean_path = format!("{}:\\", first_char);
        let wide: Vec<u16> = clean_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut serial = 0u32;

        unsafe {
            let res = GetVolumeInformationW(
                PCWSTR(wide.as_ptr()),
                None,
                Some(&mut serial),
                None,
                None,
                None,
            );
            if res.is_ok() {
                Some(serial)
            } else {
                None
            }
        }
    }

    /// 记录并立即落盘指定驱动器的 Salt
    pub fn register_salt(drive_path: &str, salt: u64) -> Result<(), String> {
        let mut reg = Self::load();
        let serial = Self::get_volume_serial(drive_path);
        let key = if let Some(s) = serial {
            format!("{:08X}", s)
        } else {
            drive_path.trim_end_matches(['\\', '/']).to_ascii_uppercase()
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let record = SaltRecord {
            drive_path: drive_path.to_string(),
            volume_serial: serial,
            salt,
            salt_hex: format!("0x{:016X}", salt),
            updated_at: now,
        };

        reg.records.insert(key, record);
        reg.save()
    }

    /// 查询指定驱动器已落盘的历史 Salt
    pub fn query_salt(drive_path: &str) -> Option<u64> {
        let reg = Self::load();
        // 1. 优先通过卷序列号高精度匹配 (杜绝插拔盘符漂移与跨盘串盐)
        if let Some(serial) = Self::get_volume_serial(drive_path) {
            let serial_key = format!("{:08X}", serial);
            if let Some(record) = reg.records.get(&serial_key) {
                return Some(record.salt);
            }
            // 如果设备存在卷序列号但在注册表中未登记，则该卷绝非以此凭证写入，严禁回退到盘符前缀模糊匹配！
            return None;
        }

        // 2. 备用：仅当无法提取卷序列号时，才尝试精确匹配标准化全路径
        let norm_path = drive_path.trim_end_matches(['\\', '/']).to_ascii_uppercase();
        for r in reg.records.values() {
            if r.volume_serial.is_none() && r.drive_path.trim_end_matches(['\\', '/']).to_ascii_uppercase() == norm_path {
                return Some(r.salt);
            }
        }

        None
    }

    /// 注销指定驱动器的记录 (用于全流程通过自动清理后)
    pub fn unregister_salt(drive_path: &str) {
        let mut reg = Self::load();
        let mut changed = false;

        if let Some(serial) = Self::get_volume_serial(drive_path) {
            let key = format!("{:08X}", serial);
            if reg.records.remove(&key).is_some() {
                changed = true;
            }
        } else {
            let norm_path = drive_path.trim_end_matches(['\\', '/']).to_ascii_uppercase();
            reg.records.retain(|_, r| {
                if r.volume_serial.is_none() && r.drive_path.trim_end_matches(['\\', '/']).to_ascii_uppercase() == norm_path {
                    changed = true;
                    false
                } else {
                    true
                }
            });
        }

        if changed {
            let _ = reg.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_salt_registry_roundtrip() {
        let mut reg = SaltRegistry::default();
        reg.records.insert(
            "TEST_KEY".to_string(),
            SaltRecord {
                drive_path: "G:\\".to_string(),
                volume_serial: Some(0x1234_5678),
                salt: 0xAABB_CCDD_EEFF_0011,
                salt_hex: "0xAABBCCDDEEFF0011".to_string(),
                updated_at: 1000000,
            },
        );

        let mut lines = Vec::new();
        for (k, r) in &reg.records {
            let serial_str = r.volume_serial.map(|s| s.to_string()).unwrap_or_default();
            lines.push(format!(
                "{}|{}|{}|{}|{}|{}",
                k, r.drive_path, serial_str, r.salt, r.salt_hex, r.updated_at
            ));
        }
        let content = lines.join("\n");
        assert!(content.contains("0xAABBCCDDEEFF0011"));
    }
}
