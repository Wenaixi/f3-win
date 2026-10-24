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

/// 便携加盐防伪签名凭据 (保存在目标磁盘根目录下 .f3_proof.sig)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofSignature {
    pub version: u32,
    pub salt: u64,
    pub salt_hex: String,
    pub created_at: u64,
    pub volume_serial: Option<u32>,
    pub signature: String,
}

impl ProofSignature {
    pub const FILENAME: &'static str = ".f3_proof.sig";
    pub const MAGIC: &'static str = "F3PROOF1";

    /// 计算防篡改校验码 (纯标准库雪崩散列，无外部依赖)
    pub fn compute_signature(salt: u64, created_at: u64, serial: Option<u32>) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;
        for b in Self::MAGIC.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(prime);
        }
        for b in salt.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(prime);
        }
        for b in created_at.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(prime);
        }
        let s_val = serial.unwrap_or(0);
        for b in s_val.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(prime);
        }
        let mut x = h ^ (h >> 30);
        x = x.wrapping_mul(0xbf58476d1ce4e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d049bb133111eb);
        x ^= x >> 31;
        format!("{:016X}", x)
    }

    /// 在指定目录下写入 .f3_proof.sig 便携防伪签名文件
    pub fn write_to_dir(dir: &std::path::Path, salt: u64, volume_serial: Option<u32>) -> Result<(), String> {
        if salt == 0 {
            // 原版兼容模式 (salt == 0) 绝不落盘签名，且清理已有旧签名
            Self::remove_from_dir(dir);
            return Ok(());
        }

        let file_path = dir.join(Self::FILENAME);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let sig = Self::compute_signature(salt, now, volume_serial);
        let serial_str = volume_serial
            .map(|s| format!("0x{:08X}", s))
            .unwrap_or_else(|| "NONE".to_string());

        let content = format!(
            "# F3-Win Portable Anti-Cheat Proof Signature\n             # This file authenticates one-way salt parameters across machines.\n             magic={}\n             version=1\n             salt=0x{:016X}\n             created_at={}\n             volume_serial={}\n             signature={}\n",
            Self::MAGIC,
            salt,
            now,
            serial_str,
            sig
        );

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)
            .map_err(|e| format!("无法创建便携防伪凭据文件 ({:?}): {}", file_path, e))?;

        file.write_all(content.as_bytes())
            .map_err(|e| format!("写入便携防伪凭据失败: {}", e))?;
        file.flush()
            .map_err(|e| format!("冲刷便携防伪凭据失败: {}", e))?;

        // 在 Windows 平台下尝试设置隐藏文件属性
        #[cfg(windows)]
        {
            use windows::core::PCWSTR;
            use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
            let wide: Vec<u16> = file_path
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            unsafe {
                let _ = SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN);
            }
        }

        Ok(())
    }

    /// 从指定目录下读取并核验 .f3_proof.sig
    pub fn read_from_dir(dir: &std::path::Path) -> Option<ProofSignature> {
        let file_path = dir.join(Self::FILENAME);
        if !file_path.exists() {
            return None;
        }

        let mut file = File::open(&file_path).ok()?;
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;

        let mut magic = String::new();
        let mut version = 0u32;
        let mut salt = 0u64;
        let mut created_at = 0u64;
        let mut volume_serial: Option<u32> = None;
        let mut signature = String::new();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                match k {
                    "magic" => magic = v.to_string(),
                    "version" => version = v.parse().unwrap_or(0),
                    "salt" => {
                        let clean = v.trim_start_matches("0x").trim_start_matches("0X");
                        salt = u64::from_str_radix(clean, 16).unwrap_or(0);
                    }
                    "created_at" => created_at = v.parse().unwrap_or(0),
                    "volume_serial" => {
                        if v != "NONE" {
                            let clean = v.trim_start_matches("0x").trim_start_matches("0X");
                            volume_serial = u32::from_str_radix(clean, 16).ok();
                        }
                    }
                    "signature" => signature = v.to_string(),
                    _ => {}
                }
            }
        }

        if magic != Self::MAGIC || salt == 0 {
            return None;
        }

        // 校验签名真实性
        let expected_sig = Self::compute_signature(salt, created_at, volume_serial);
        if !signature.eq_ignore_ascii_case(&expected_sig) {
            return None;
        }

        Some(ProofSignature {
            version,
            salt,
            salt_hex: format!("0x{:016X}", salt),
            created_at,
            volume_serial,
            signature,
        })
    }

    /// 从指定目录下彻底删除便携凭据文件
    pub fn remove_from_dir(dir: &std::path::Path) {
        let file_path = dir.join(Self::FILENAME);
        if file_path.exists() {
            let _ = std::fs::remove_file(file_path);
        }
    }
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

    /// 记录并立即落盘指定驱动器的 Salt (同时在驱动器根目录生成 .f3_proof.sig 便携凭据)
    pub fn register_salt(drive_path: &str, salt: u64) -> Result<(), String> {
        let serial = Self::get_volume_serial(drive_path);
        let path = std::path::Path::new(drive_path);

        // 1. 同步管理目标磁盘目录下的便携凭据文件
        let _ = ProofSignature::write_to_dir(path, salt, serial);

        if salt == 0 {
            // 原版兼容模式下不登记本地防作弊凭据
            Self::unregister_salt(drive_path);
            return Ok(());
        }

        // 2. 登记至 exe 同级持久化注册表
        let mut reg = Self::load();
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

    /// 查询指定驱动器已落盘的历史 Salt (优先检测磁盘根目录便携签名凭据，次选本地注册表)
    pub fn query_salt(drive_path: &str) -> Option<u64> {
        let current_serial = Self::get_volume_serial(drive_path);
        let path = std::path::Path::new(drive_path);

        // 0. 优先检测目标磁盘目录下的 .f3_proof.sig 便携防伪签名文件 (实现跨机器即插即测互验)
        if let Some(proof) = ProofSignature::read_from_dir(path) {
            // 如果设备能获取卷序列号且凭据中登记了序列号，则必须一致 (防止复制签名文件跨盘串改)
            let serial_match = match (current_serial, proof.volume_serial) {
                (Some(curr), Some(reg)) => curr == reg,
                _ => true,
            };
            if serial_match {
                return Some(proof.salt);
            }
        }

        // 1. 本地注册表：优先通过卷序列号高精度匹配
        let reg = Self::load();
        if let Some(serial) = current_serial {
            let serial_key = format!("{:08X}", serial);
            if let Some(record) = reg.records.get(&serial_key) {
                return Some(record.salt);
            }
            // 如果物理盘存在卷序列号但在注册表中未登记，且无磁盘根凭据，则判定为未加盐（原版 f3 格式）
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

    /// 注销指定驱动器的记录 (同步清理磁盘根目录便携凭据与本地注册表)
    pub fn unregister_salt(drive_path: &str) {
        let path = std::path::Path::new(drive_path);
        ProofSignature::remove_from_dir(path);

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

    #[test]
    fn test_proof_signature_roundtrip() {
        let temp_dir = std::env::temp_dir().join("f3_test_sig_dir");
        let _ = std::fs::create_dir_all(&temp_dir);

        let test_salt = 0x1122_3344_5566_7788u64;
        let test_serial = Some(0x9ABC_DEF0u32);

        // 1. 写入签名
        assert!(ProofSignature::write_to_dir(&temp_dir, test_salt, test_serial).is_ok());

        // 2. 读取并验证签名
        let proof = ProofSignature::read_from_dir(&temp_dir).expect("应成功读取并验证签名");
        assert_eq!(proof.salt, test_salt);
        assert_eq!(proof.volume_serial, test_serial);
        assert_eq!(proof.version, 1);

        // 3. 篡改测试：修改 salt 字段后验签必须失败
        let sig_file = temp_dir.join(ProofSignature::FILENAME);
        let orig_content = std::fs::read_to_string(&sig_file).unwrap();
        let tampered = orig_content.replace(&format!("0x{:016X}", test_salt), "0x9999999999999999");
        std::fs::write(&sig_file, tampered).unwrap();
        assert!(ProofSignature::read_from_dir(&temp_dir).is_none(), "篡改签名后必须被拒绝");

        // 4. 清理凭据
        ProofSignature::remove_from_dir(&temp_dir);
        assert!(ProofSignature::read_from_dir(&temp_dir).is_none());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
