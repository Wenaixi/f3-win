use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY,
    DISK_GEOMETRY_EX, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
    PropertyStandardQuery, StorageDeviceProperty,
};

const DRIVE_REMOVABLE: u32 = 2;
const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D1080;

#[repr(C)]
#[derive(Default)]
struct STORAGE_DEVICE_NUMBER {
    device_type: u32,
    device_number: u32,
    partition_number: u32,
}

// 物理总线类型定义
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
            StorageBusType::Unknown(_) => "其他总线",
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
    pub mounted_volumes: Vec<char>,
}

impl PhysicalDeviceInfo {
    // 枚举系统物理存储磁盘 (PhysicalDrive0 ~ 31)，使用 0 权限查询，确保不被 Access Denied
    pub fn enumerate_devices() -> Vec<PhysicalDeviceInfo> {
        let mut devices = Vec::new();
        let volumes = VolumeInfo::enumerate_volumes();

        for i in 0..32 {
            let path_str = format!(r"\\.\PhysicalDrive{}", i);
            let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();

            // 以 0 (Query access) 权限打开，避免普通用户权限下被 Windows 安全机制报 Error 5
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    HANDLE::default(),
                )
            };

            if let Ok(h) = handle {
                if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
                    if let Some(mut info) = Self::query_device_info(i, &path_str, h) {
                        // 关联该物理磁盘上挂载的全部逻辑盘符
                        for vol in &volumes {
                            if let Some(parent_disk) = get_physical_drive_number_for_volume(vol.drive_letter) {
                                if parent_disk == i && !info.mounted_volumes.contains(&vol.drive_letter) {
                                    info.mounted_volumes.push(vol.drive_letter);
                                }
                            }
                        }
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
        let is_safe = (bus_type.is_removable_bus() || is_removable) && index != 0;
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
            mounted_volumes: Vec::new(),
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
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => {
                let mut dummy = 0u32;
                unsafe {
                    // 1. 尝试锁定卷
                    let _ = windows::Win32::System::IO::DeviceIoControl(
                        h,
                        FSCTL_LOCK_VOLUME,
                        None,
                        0,
                        None,
                        0,
                        Some(&mut dummy),
                        None,
                    );

                    // 2. 卸载卷文件系统挂载
                    let _ = windows::Win32::System::IO::DeviceIoControl(
                        h,
                        FSCTL_DISMOUNT_VOLUME,
                        None,
                        0,
                        None,
                        0,
                        Some(&mut dummy),
                        None,
                    );
                }
                Ok(h)
            }
            _ => Err(format!("无法打开卷 {}:，请确认管理员权限", drive_letter)),
        }
    }
}

// 通过 IOCTL_STORAGE_GET_DEVICE_NUMBER 从盘符反查底层物理磁盘编号
pub fn get_physical_drive_number_for_volume(drive_letter: char) -> Option<u32> {
    let vol_str = format!(r"\\.\{}:", drive_letter);
    let mut wide: Vec<u16> = vol_str.encode_utf16().collect();
    wide.push(0);

    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0, // 仅属性查询，免去权限阻碍
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
    };

    if let Ok(h) = handle {
        if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
            let mut sdn = STORAGE_DEVICE_NUMBER::default();
            let mut returned = 0u32;
            let ok = unsafe {
                windows::Win32::System::IO::DeviceIoControl(
                    h,
                    IOCTL_STORAGE_GET_DEVICE_NUMBER,
                    None,
                    0,
                    Some(&mut sdn as *mut _ as *mut c_void),
                    size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                    Some(&mut returned),
                    None,
                )
            };
            unsafe { let _ = CloseHandle(h); }
            if ok.is_ok() {
                return Some(sdn.device_number);
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub drive_letter: char,
    pub root_path: PathBuf,
    pub label: String,
    pub file_system: String,
    pub is_removable: bool,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

impl VolumeInfo {
    pub fn display_name(&self) -> String {
        let kind = if self.is_removable {
            "可移动 U 盘"
        } else {
            "本地硬盘"
        };
        let label_part = if !self.label.is_empty() {
            format!(" [{}]", self.label)
        } else {
            String::new()
        };
        format!(
            "[{}] {}:\\{} ({}) - {:.1} GB 可用 / {:.1} GB 总计",
            kind,
            self.drive_letter,
            label_part,
            self.file_system,
            self.free_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            self.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        )
    }

    pub fn enumerate_volumes() -> Vec<VolumeInfo> {
        let mut volumes = Vec::new();
        let mask = unsafe { GetLogicalDrives() };

        for i in 0..26 {
            if (mask & (1 << i)) != 0 {
                let letter = (b'A' + i as u8) as char;
                let root_str = format!("{}:\\\0", letter);
                let wide: Vec<u16> = root_str.encode_utf16().collect();

                let drive_type = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };
                if drive_type == 0 || drive_type == 1 {
                    continue;
                }

                let is_removable = drive_type == DRIVE_REMOVABLE;

                let mut vol_name = [0u16; 260];
                let mut fs_name = [0u16; 260];
                let _ = unsafe {
                    GetVolumeInformationW(
                        PCWSTR(wide.as_ptr()),
                        Some(&mut vol_name),
                        None,
                        None,
                        None,
                        Some(&mut fs_name),
                    )
                };

                let label = String::from_utf16_lossy(&vol_name)
                    .trim_matches('\0')
                    .trim()
                    .to_string();
                let file_system = String::from_utf16_lossy(&fs_name)
                    .trim_matches('\0')
                    .trim()
                    .to_string();

                let mut free_avail = 0u64;
                let mut total_bytes = 0u64;
                let mut total_free = 0u64;
                let _ = unsafe {
                    GetDiskFreeSpaceExW(
                        PCWSTR(wide.as_ptr()),
                        Some(&mut free_avail),
                        Some(&mut total_bytes),
                        Some(&mut total_free),
                    )
                };

                volumes.push(VolumeInfo {
                    drive_letter: letter,
                    root_path: PathBuf::from(format!("{}:\\", letter)),
                    label,
                    file_system,
                    is_removable,
                    free_bytes: free_avail,
                    total_bytes,
                });
            }
        }

        volumes.sort_by_key(|v| !v.is_removable);
        volumes
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
