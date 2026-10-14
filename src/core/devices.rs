use std::ffi::c_void;
use std::mem::size_of;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY,
    DISK_GEOMETRY_EX, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    PropertyStandardQuery, StorageDeviceProperty,
};

// 物理总线类型定义 (与 Windows DDK 对应)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBusType {
    Usb,
    Sd,
    Mmc,
    Sata,
    Nvme,
    Unknown(u32),
}

impl StorageBusType {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            7 => StorageBusType::Usb,
            11 => StorageBusType::Sata,
            12 => StorageBusType::Sd,
            13 => StorageBusType::Mmc,
            17 => StorageBusType::Nvme,
            other => StorageBusType::Unknown(other),
        }
    }

    pub fn is_removable_bus(&self) -> bool {
        matches!(self, StorageBusType::Usb | StorageBusType::Sd | StorageBusType::Mmc)
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            StorageBusType::Usb => "USB",
            StorageBusType::Sd => "SD 卡",
            StorageBusType::Mmc => "MMC",
            StorageBusType::Sata => "SATA (内部磁盘)",
            StorageBusType::Nvme => "NVMe (内部磁盘)",
            StorageBusType::Unknown(_) => "其他未知总线",
        }
    }
}

// 物理设备描述结构
#[derive(Debug, Clone)]
pub struct PhysicalDeviceInfo {
    pub drive_index: u32,
    pub path: String,
    pub vendor_id: String,
    pub product_id: String,
    pub serial_number: String,
    pub bus_type: StorageBusType,
    pub total_bytes: u64,
    pub is_removable: bool,
    pub is_safe_target: bool,
    pub warning: Option<String>,
}

impl PhysicalDeviceInfo {
    // 枚举系统当前连接的物理存储磁盘 (PhysicalDrive0 ~ 31)
    pub fn enumerate_devices() -> Vec<PhysicalDeviceInfo> {
        let mut devices = Vec::new();

        for i in 0..32 {
            let path_str = format!(r"\\.\PhysicalDrive{}", i);
            let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();

            let handle = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    FILE_GENERIC_READ.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    HANDLE::default(),
                )
            };

            if let Ok(h) = handle {
                if h != INVALID_HANDLE_VALUE {
                    if let Some(info) = Self::query_device_info(i, &path_str, h) {
                        devices.push(info);
                    }
                    unsafe { let _ = CloseHandle(h); }
                }
            }
        }

        devices
    }

    fn query_device_info(index: u32, path: &str, handle: HANDLE) -> Option<Self> {
        // 1. 查询磁盘几何结构与总容量
        let mut geometry = DISK_GEOMETRY_EX::default();
        let mut bytes_returned = 0u32;

        let ok_geom = unsafe {
            windows::Win32::System::IO::DeviceIoControl(
                handle,
                IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
                None,
                0,
                Some(&mut geometry as *mut _ as *mut c_void),
                size_of::<DISK_GEOMETRY_EX>() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        let total_bytes = if ok_geom.is_ok() {
            geometry.DiskSize as u64
        } else {
            0u64
        };

        // 2. 查询总线属性
        let mut query = STORAGE_PROPERTY_QUERY::default();
        query.PropertyId = StorageDeviceProperty;
        query.QueryType = PropertyStandardQuery;

        let mut buffer = [0u8; 1024];
        let ok_prop = unsafe {
            windows::Win32::System::IO::DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(&query as *const _ as *const c_void),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                Some(buffer.as_mut_ptr() as *mut c_void),
                buffer.len() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        if ok_prop.is_err() {
            return None;
        }

        let desc = unsafe { &*(buffer.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
        let bus_type = StorageBusType::from_raw(desc.BusType.0 as u32);
        let is_removable = desc.RemovableMedia.as_bool();

        let vendor_id = extract_sz(&buffer, desc.VendorIdOffset as usize);
        let product_id = extract_sz(&buffer, desc.ProductIdOffset as usize);
        let serial = extract_sz(&buffer, desc.SerialNumberOffset as usize);

        // 安全判定：只有外部移动介质且非系统主盘才算安全测试目标
        let is_safe = bus_type.is_removable_bus() && index != 0;
        let warning = if !is_safe {
            Some("系统内部固定磁盘已硬隔离锁定，严禁运行破坏性扇区探测".to_string())
        } else {
            None
        };

        Some(PhysicalDeviceInfo {
            drive_index: index,
            path: path.to_string(),
            vendor_id,
            product_id,
            serial_number: serial,
            bus_type,
            total_bytes,
            is_removable,
            is_safe_target: is_safe,
            warning,
        })
    }

    // 尝试锁定并卸载该磁盘上所有卷的挂载点
    pub fn lock_and_dismount_volume(drive_letter: char) -> Result<HANDLE, String> {
        let vol_path = format!(r"\\.\{}:", drive_letter);
        let wide: Vec<u16> = vol_path.encode_utf16().chain(std::iter::once(0)).collect();

        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                HANDLE::default(),
            )
        };

        match handle {
            Ok(h) if h != INVALID_HANDLE_VALUE => {
                let mut dummy = 0u32;
                unsafe {
                    // 1. 尝试锁定卷
                    let lock_res = windows::Win32::System::IO::DeviceIoControl(
                        h,
                        FSCTL_LOCK_VOLUME,
                        None,
                        0,
                        None,
                        0,
                        Some(&mut dummy),
                        None,
                    );
                    if lock_res.is_err() {
                        let _ = CloseHandle(h);
                        return Err(format!("锁定卷 {}: 失败，可能正被其他进程占用", drive_letter));
                    }

                    // 2. 卸载卷文件系统挂载
                    let dismount_res = windows::Win32::System::IO::DeviceIoControl(
                        h,
                        FSCTL_DISMOUNT_VOLUME,
                        None,
                        0,
                        None,
                        0,
                        Some(&mut dummy),
                        None,
                    );
                    if dismount_res.is_err() {
                        let _ = windows::Win32::System::IO::DeviceIoControl(
                            h,
                            FSCTL_UNLOCK_VOLUME,
                            None,
                            0,
                            None,
                            0,
                            Some(&mut dummy),
                            None,
                        );
                        let _ = CloseHandle(h);
                        return Err(format!("卸载卷 {}: 失败", drive_letter));
                    }
                }
                Ok(h)
            }
            _ => Err(format!("无法打开卷 {}:，请以管理员身份运行", drive_letter)),
        }
    }
}

fn extract_sz(buf: &[u8], offset: usize) -> String {
    if offset == 0 || offset >= buf.len() {
        return String::new();
    }
    let slice = &buf[offset..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).trim().to_string()
}