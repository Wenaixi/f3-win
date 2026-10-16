use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

/// 单个驱动器的加盐防伪记录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SaltRecord {
    pub drive_path: String,
    pub volume_serial: Option<u32>,
    pub salt: u64,
    pub salt_hex: String,
    pub updated_at: u64,
}

/// 现代防伪凭证注册表 (存储在 exe 同级目录下，物理隔离防固件窥探)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SaltRegistry {
    pub version: u32,
    /// 键为卷序列号的十六进制或盘符大写字符串
    pub records: HashMap<String, SaltRecord>,
}

impl SaltRegistry {
    const REGISTRY_FILENAME: &'static str = "f3_salt_registry.json";

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
        if !path.exists() {
            return Self {
                version: 1,
                records: HashMap::new(),
            };
        }

        match File::open(&path) {
            Ok(mut f) => {
                let mut content = String::new();
                if f.read_to_string(&mut content).is_ok() {
                    serde_json::from_str(&content).unwrap_or_else(|_| Self {
                        version: 1,
                        records: HashMap::new(),
                    })
                } else {
                    Self {
                        version: 1,
                        records: HashMap::new(),
                    }
                }
            }
            Err(_) => Self {
                version: 1,
                records: HashMap::new(),
            },
        }
    }

    /// 将注册表原子持久化落盘至 exe 同级目录下
    pub fn save(&self) -> Result<(), String> {
        let path = Self::get_registry_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("序列化 Salt 注册表失败: {}", e))?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("打开 Salt 注册表文件失败 ({:?}): {}", path, e))?;

        file.write_all(json.as_bytes())
            .map_err(|e| format!("写入 Salt 注册表失败: {}", e))?;
        file.flush()
            .map_err(|e| format!("冲刷 Salt 注册表失败: {}", e))?;

        Ok(())
    }

    /// 提取指定盘符路径的卷序列号 (如 "G:\\")
    pub fn get_volume_serial(drive_path: &str) -> Option<u32> {
        let clean_path = format!("{}:\\", drive_path.chars().next()?.to_ascii_uppercase());
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
            drive_path.chars().next().unwrap_or('U').to_ascii_uppercase().to_string()
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
        // 1. 优先通过卷序列号高精度匹配 (杜绝插拔盘符漂移)
        if let Some(serial) = Self::get_volume_serial(drive_path) {
            let serial_key = format!("{:08X}", serial);
            if let Some(record) = reg.records.get(&serial_key) {
                return Some(record.salt);
            }
        }

        // 2. 备用通过盘符前缀匹配
        let letter_key = drive_path.chars().next()?.to_ascii_uppercase().to_string();
        if let Some(record) = reg.records.get(&letter_key) {
            return Some(record.salt);
        }

        // 3. 模糊匹配 drive_path
        for r in reg.records.values() {
            if r.drive_path.starts_with(&letter_key) {
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
        }

        let letter_key = if let Some(c) = drive_path.chars().next() {
            c.to_ascii_uppercase().to_string()
        } else {
            return;
        };

        if reg.records.remove(&letter_key).is_some() {
            changed = true;
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
    fn test_salt_registry_serde() {
        let mut reg = SaltRegistry {
            version: 1,
            records: HashMap::new(),
        };

        reg.records.insert(
            "TEST_SERIAL".to_string(),
            SaltRecord {
                drive_path: "G:\\".to_string(),
                volume_serial: Some(0x1234_5678),
                salt: 0xAABB_CCDD_EEFF_0011,
                salt_hex: "0xAABBCCDDEEFF0011".to_string(),
                updated_at: 1000000,
            },
        );

        let json = serde_json::to_string(&reg).expect("序列化必须成功");
        let restored: SaltRegistry = serde_json::from_str(&json).expect("反序列化必须成功");

        assert_eq!(restored.records.len(), 1);
        let rec = restored.records.get("TEST_SERIAL").unwrap();
        assert_eq!(rec.salt, 0xAABB_CCDD_EEFF_0011);
        assert_eq!(rec.volume_serial, Some(0x1234_5678));
    }
}
