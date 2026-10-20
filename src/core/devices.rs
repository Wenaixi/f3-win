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
use crate::core::direct_io::DirectIoFile;

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
            StorageBusType::Sata => "SATA (内部固定磁盘)",
            StorageBusType::Nvme => "NVMe (内部固定磁盘)",
            StorageBusType::Unknown(_) => "其他未知总线",
        }
    }
}

// 卷独占锁 RAII 守卫：持有一组已锁定的卷句柄，生命周期结束时自动安全关闭句柄释放独占锁
pub struct VolumeLockGuard {
    handles: Vec<HANDLE>,
    pub locked_volumes: Vec<char>,
}

unsafe impl Send for VolumeLockGuard {}
unsafe impl Sync for VolumeLockGuard {}

impl Drop for VolumeLockGuard {
    fn drop(&mut self) {
        // 当探测完成、出错退出或线程结束时，自动遍历关闭句柄，通知 Windows 恢复挂载
        for &h in &self.handles {
            if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        }
    }
}

// 物理设备描述结构体
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
    pub fn display_name(&self) -> String {
        let name = format!("{} {}", self.vendor_id, self.product_id).trim().to_string();
        let name = if name.is_empty() { "通用存储设备".to_string() } else { name };
        let gb = self.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        let bus = self.bus_type.display_name();

        let vol_str = if !self.mounted_volumes.is_empty() {
            let letters: Vec<String> = self.mounted_volumes.iter().map(|c| format!("{}:", c)).collect();
            format!(" [{}]", letters.join(", "))
        } else {
            String::new()
        };

        format!("PhysicalDrive{} ({:.1} GB, {}) - {}{}", self.drive_index, gb, bus, name, vol_str)
    }

    // 针对指定物理磁盘编号即时查询其硬件属性与总线类型（破坏性操作前复核）
    pub fn query_by_index(index: u32) -> Option<Self> {
        let path_str = format!(r"\\.\PhysicalDrive{}", index);
        let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();

        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                0, // 仅属性查询，免去特权阻碍
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                HANDLE::default(),
            )
        };

        if let Ok(h) = handle {
            if h != INVALID_HANDLE_VALUE && !h.is_invalid() {
                let info = Self::query_device_info(index, &path_str, h);
                unsafe { let _ = CloseHandle(h); }
                return info;
            }
        }
        None
    }

    // 全盘枚举系统中的所有物理存储设备 (0..16)
    pub fn enumerate_devices() -> Vec<PhysicalDeviceInfo> {
        let mut devices = Vec::new();
        for index in 0..16 {
            if let Some(dev) = Self::query_by_index(index) {
                devices.push(dev);
            }
        }

        // 将检测到的逻辑卷反查归类到对应的物理磁盘上
        let volumes = VolumeInfo::enumerate_volumes();
        for vol in volumes {
            if let Some(disk_num) = get_physical_drive_number_for_volume(vol.drive_letter) {
                if let Some(dev) = devices.iter_mut().find(|d| d.drive_index == disk_num) {
                    if !dev.mounted_volumes.contains(&vol.drive_letter) {
                        dev.mounted_volumes.push(vol.drive_letter);
                    }
                }
            }
        }

        devices.sort_by_key(|d| (!d.is_safe_target, d.drive_index));
        devices
    }

    fn query_device_info(index: u32, path: &str, handle: HANDLE) -> Option<Self> {
        // 1. 查询磁盘几何容量
        let mut geom = DISK_GEOMETRY_EX::default();
        let mut bytes_returned = 0u32;
        let ok_geom = unsafe {
            windows::Win32::System::IO::DeviceIoControl(
                handle,
                IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
                None,
                0,
                Some(&mut geom as *mut _ as *mut c_void),
                size_of::<DISK_GEOMETRY_EX>() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        let total_bytes = if ok_geom.is_ok() {
            geom.DiskSize as u64
        } else {
            0
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

        // 安全判定：只有外部移动介质且非系统主盘/引导盘才算安全测试目标
        let sys_disk_idx = get_system_drive_disk_number();
        let is_sys_disk = index == 0 || sys_disk_idx == Some(index);
        let is_safe = (bus_type.is_removable_bus() || is_removable) && !is_sys_disk;
        let warning = if !is_safe {
            if is_sys_disk {
                Some("系统主盘/引导盘已被绝对硬隔离锁定，严禁运行破坏性扇区探测".to_string())
            } else {
                Some("系统内部固定磁盘已硬隔离锁定，严禁运行破坏性扇区探测".to_string())
            }
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

    // 尝试锁定并卸载单个卷（内置 3 次重试以吸收后台索引器瞬时占用）
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

        let h = match handle {
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => h,
            _ => return Err(format!("无法打开卷 {}:，请确认具备管理员权限", drive_letter)),
        };

        let mut dummy = 0u32;
        let mut locked = false;

        // 尝试锁定卷 (带 3 次退避重试)
        for attempt in 0..3 {
            let res = unsafe {
                windows::Win32::System::IO::DeviceIoControl(
                    h,
                    FSCTL_LOCK_VOLUME,
                    None,
                    0,
                    None,
                    0,
                    Some(&mut dummy),
                    None,
                )
            };
            if res.is_ok() {
                locked = true;
                break;
            }
            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }

        if !locked {
            unsafe { let _ = CloseHandle(h); }
            return Err(format!("卷 {}: 被其他程序占用，无法独占锁定", drive_letter));
        }

        // 卸载卷文件系统挂载
        let _ = unsafe {
            windows::Win32::System::IO::DeviceIoControl(
                h,
                FSCTL_DISMOUNT_VOLUME,
                None,
                0,
                None,
                0,
                Some(&mut dummy),
                None,
            )
        };

        Ok(h)
    }
}

// 存储目标深模块 (StorageTarget)
// 封装设备属性、总线安全隔离与独占卷锁准备流程
#[derive(Debug, Clone)]
pub struct StorageTarget {
    pub device: PhysicalDeviceInfo,
    pub volumes: Vec<VolumeInfo>,
}

impl StorageTarget {
    pub fn new(device: PhysicalDeviceInfo, volumes: Vec<VolumeInfo>) -> Self {
        Self { device, volumes }
    }

    pub fn is_safe_for_raw_probe(&self) -> bool {
        // 读取系统引导盘符 (默认 C:)
        let sys_drive = std::env::var("SystemDrive")
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('C')
            .to_ascii_uppercase();

        let contains_system_drive = self.device.mounted_volumes.iter().any(|&c| c.to_ascii_uppercase() == sys_drive)
            || self.volumes.iter().any(|v| v.drive_letter.to_ascii_uppercase() == sys_drive);

        let sys_disk_idx = get_system_drive_disk_number();
        let is_sys_disk = self.device.drive_index == 0 || sys_disk_idx == Some(self.device.drive_index);

        // 绝对安全熔断：必须非系统主盘/引导盘、通过总线白名单、且不包含系统引导卷 (C:)
        self.device.is_safe_target && !is_sys_disk && !contains_system_drive
    }

    // 执行物理探测前置准备：验证安全性，并独占锁定该磁盘上的全部挂载卷
    pub fn prepare_for_probe(&self) -> Result<VolumeLockGuard, String> {
        // Zero-TOCTOU 零时差现场硬件总线复核：杜绝拔插时间差导致的物理设备漂移
        let live_dev = PhysicalDeviceInfo::query_by_index(self.device.drive_index)
            .ok_or_else(|| format!("目标物理磁盘 PhysicalDrive{} 已断开连接", self.device.drive_index))?;

        let sys_disk_idx = get_system_drive_disk_number();
        let is_sys_disk = live_dev.drive_index == 0 || sys_disk_idx == Some(live_dev.drive_index);
        if !live_dev.is_safe_target || is_sys_disk {
            return Err(format!(
                "安全硬隔离拦截：PhysicalDrive{} 未通过现场硬件总线核验 (系统主盘/引导盘或内部固定介质)",
                live_dev.drive_index
            ));
        }

        if !self.is_safe_for_raw_probe() {
            return Err(format!(
                "目标磁盘 PhysicalDrive{} 未通过安全核验 (系统主盘或固定磁盘已锁定)",
                self.device.drive_index
            ));
        }

        let mut handles = Vec::new();
        let mut locked_volumes = Vec::new();

        for &letter in &self.device.mounted_volumes {
            match PhysicalDeviceInfo::lock_and_dismount_volume(letter) {
                Ok(h) => {
                    handles.push(h);
                    locked_volumes.push(letter);
                }
                Err(e) => {
                    // 若任何一个卷锁定失败，回滚并释放已锁定的卷句柄
                    for h in handles {
                        unsafe { let _ = CloseHandle(h); }
                    }
                    return Err(format!("准备探测失败: {}", e));
                }
            }
        }

        Ok(VolumeLockGuard {
            handles,
            locked_volumes,
        })
    }

    // 一站式打开底层物理磁盘并独占锁定卸载其挂载的所有逻辑卷
    // 返回 DirectIoFile (满足 BlockIo 接缝) 与 RAII 独占锁守卫 VolumeLockGuard
    pub fn open_block_io(&self) -> Result<(DirectIoFile, VolumeLockGuard), String> {
        let guard = self.prepare_for_probe()?;
        let path = std::path::Path::new(&self.device.path);
        match DirectIoFile::open_raw_device(path) {
            Ok(file) => Ok((file, guard)),
            Err(e) => Err(format!("打开底层物理设备失败 (需要管理员权限): {}", e)),
        }
    }
}

// 存储设备管理器 (StorageDeviceManager)
#[derive(Debug, Clone, Default)]
pub struct StorageDeviceManager {
    pub devices: Vec<PhysicalDeviceInfo>,
    pub volumes: Vec<VolumeInfo>,
}

impl StorageDeviceManager {
    pub fn new() -> Self {
        let mut mgr = Self::default();
        mgr.refresh();
        mgr
    }

    pub fn refresh(&mut self) {
        self.devices = PhysicalDeviceInfo::enumerate_devices();
        self.volumes = VolumeInfo::enumerate_volumes();
    }

    pub fn get_target_for_device(&self, device_index: u32) -> Option<StorageTarget> {
        let dev = self.devices.iter().find(|d| d.drive_index == device_index)?.clone();
        let child_vols = self.volumes.iter()
            .filter(|v| dev.mounted_volumes.contains(&v.drive_letter))
            .cloned()
            .collect();
        Some(StorageTarget::new(dev, child_vols))
    }

    pub fn get_target_for_volume(&self, letter: char) -> Option<StorageTarget> {
        let disk_num = get_physical_drive_number_for_volume(letter)?;
        self.get_target_for_device(disk_num)
    }
}

// 动态反查系统引导盘盘符 (优先 SystemDrive，备选 SystemRoot / windir，兜底 'C')
pub fn get_system_drive_letter() -> char {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|s| s.chars().next())
        .or_else(|| {
            std::env::var("SystemRoot")
                .ok()
                .and_then(|s| s.chars().next())
        })
        .or_else(|| {
            std::env::var("windir")
                .ok()
                .and_then(|s| s.chars().next())
        })
        .unwrap_or('C')
        .to_ascii_uppercase()
}

// 动态反查系统引导盘 (%SystemDrive% 通常为 C:) 所依附的底层物理磁盘编号
pub fn get_system_drive_disk_number() -> Option<u32> {
    let sys_drive = get_system_drive_letter();
    get_physical_drive_number_for_volume(sys_drive)
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
    pub is_safe_target: bool,
}

impl VolumeInfo {
    pub fn display_name(&self) -> String {
        let kind = if !self.is_safe_target {
            "系统或内部磁盘[锁定]"
        } else if self.is_removable {
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
            "[{}] {}:\{} ({}) - {:.1} GB 可用 / {:.1} GB 总计",
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
                let root_str = format!("{}:\\0", letter);
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

                let sys_letter = get_system_drive_letter();
                let is_sys_letter = letter == sys_letter || letter == 'C';
                let disk_num = get_physical_drive_number_for_volume(letter);
                let sys_disk_idx = get_system_drive_disk_number();
                let is_sys_disk = disk_num == Some(0) || (sys_disk_idx.is_some() && disk_num == sys_disk_idx);

                let is_safe = if is_sys_letter || is_sys_disk {
                    false
                } else if let Some(d_idx) = disk_num {
                    if let Some(dev_info) = PhysicalDeviceInfo::query_by_index(d_idx) {
                        dev_info.is_safe_target
                    } else {
                        is_removable
                    }
                } else {
                    is_removable
                };

                volumes.push(VolumeInfo {
                    drive_letter: letter,
                    root_path: PathBuf::from(format!("{}:\", letter)),
                    label,
                    file_system,
                    is_removable,
                    free_bytes: free_avail,
                    total_bytes,
                    is_safe_target: is_safe,
                });
            }
        }

        volumes.sort_by_key(|v| (!v.is_safe_target, !v.is_removable));
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
// 存储设备资产清单 (StorageInventory)
// 深层领域模块：内聚管理逻辑卷、物理磁盘、安全移动介质白名单以及卷与底层的双向关联
#[derive(Debug, Clone, Default)]
pub struct StorageInventory {
    volumes: Vec<VolumeInfo>,
    devices: Vec<PhysicalDeviceInfo>,
    selected_volume_index: usize,
    selected_device_index: Option<usize>,
}

impl StorageInventory {
    // 实例化并初始化枚举，自动优选首个安全可移动设备及对应卷
    pub fn new() -> Self {
        let mut inv = Self {
            volumes: VolumeInfo::enumerate_volumes(),
            devices: PhysicalDeviceInfo::enumerate_devices(),
            selected_volume_index: 0,
            selected_device_index: None,
        };
        inv.auto_bind_initial_selection();
        inv
    }

    // 刷新系统设备与卷列表，尽量维持当前选中的设备与卷
    pub fn refresh(&mut self) {
        let prev_volume_letter = self.selected_volume().map(|v| v.drive_letter);
        let prev_device_index = self.selected_device().map(|d| d.drive_index);

        self.volumes = VolumeInfo::enumerate_volumes();
        self.devices = PhysicalDeviceInfo::enumerate_devices();

        // 尝试恢复先前的卷选中
        if let Some(letter) = prev_volume_letter {
            if let Some(pos) = self.volumes.iter().position(|v| v.drive_letter == letter) {
                self.selected_volume_index = pos;
            } else if !self.volumes.is_empty() {
                self.selected_volume_index = 0;
            }
        } else if !self.volumes.is_empty() {
            self.selected_volume_index = 0;
        }

        // 尝试恢复先前的物理磁盘选中，或根据当前卷重新联动绑定
        if let Some(disk_idx) = prev_device_index {
            self.selected_device_index = self.devices.iter().position(|d| d.drive_index == disk_idx && d.is_safe_target);
        }
        if self.selected_device_index.is_none() {
            self.sync_device_with_selected_volume();
        }
    }

    // 自动联动：选择逻辑卷时，自动反查并选中其底层的物理磁盘 (受安全白名单保护)
    pub fn select_volume(&mut self, index: usize) {
        if index < self.volumes.len() && self.volumes[index].is_safe_target {
            self.selected_volume_index = index;
            self.sync_device_with_selected_volume();
        }
    }

    // 选择指定物理设备
    pub fn select_device(&mut self, index: usize) {
        if index < self.devices.len() {
            self.selected_device_index = Some(index);
        }
    }

    // 获取当前选中的逻辑卷
    pub fn selected_volume(&self) -> Option<&VolumeInfo> {
        self.volumes.get(self.selected_volume_index)
    }

    pub fn selected_volume_index(&self) -> usize {
        self.selected_volume_index
    }

    // 获取当前选中的物理磁盘
    pub fn selected_device(&self) -> Option<&PhysicalDeviceInfo> {
        self.selected_device_index.and_then(|i| self.devices.get(i))
    }

    pub fn selected_device_index(&self) -> Option<usize> {
        self.selected_device_index
    }

    // 获取当前选中的测试路径（根目录）
    pub fn selected_path(&self) -> PathBuf {
        self.selected_volume()
            .map(|v| v.root_path.clone())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn volumes(&self) -> &[VolumeInfo] {
        &self.volumes
    }

    pub fn devices(&self) -> &[PhysicalDeviceInfo] {
        &self.devices
    }

    // 构建测试目标 StorageTarget，自动附带该物理磁盘上挂载的全部逻辑卷以备独占卸载
    pub fn active_target(&self) -> Option<StorageTarget> {
        let dev = self.selected_device()?;
        if !dev.is_safe_target {
            return None;
        }

        // 收集该物理磁盘下的所有卷
        let target_vols: Vec<VolumeInfo> = self.volumes.iter()
            .filter(|v| get_physical_drive_number_for_volume(v.drive_letter) == Some(dev.drive_index))
            .cloned()
            .collect();

        Some(StorageTarget::new(dev.clone(), target_vols))
    }

    // 内部初次启动自动选优 (优先选择合规安全的移动介质卷)
    fn auto_bind_initial_selection(&mut self) {
        if let Some(pos) = self.volumes.iter().position(|v| v.is_safe_target) {
            self.selected_volume_index = pos;
        } else if !self.volumes.is_empty() {
            self.selected_volume_index = 0;
        }
        self.sync_device_with_selected_volume();

        // 如果依然没有匹配到物理磁盘，选第一个安全移动介质
        if self.selected_device_index.is_none() {
            self.selected_device_index = self.devices.iter().position(|d| d.is_safe_target);
        }
    }

    // 内部根据当前选中的卷反查物理磁盘并更新
    fn sync_device_with_selected_volume(&mut self) {
        if let Some(vol) = self.selected_volume() {
            if let Some(drive_num) = get_physical_drive_number_for_volume(vol.drive_letter) {
                if let Some(pos) = self.devices.iter().position(|d| d.drive_index == drive_num && d.is_safe_target) {
                    self.selected_device_index = Some(pos);
                    return;
                }
            }
        }
        // 如果当前卷无法反查（或属于非移动硬盘安全隔离），保持原状或优先移动设备
        if self.selected_device_index.is_none() {
            self.selected_device_index = self.devices.iter().position(|d| d.is_safe_target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_inventory_defaults() {
        let inv = StorageInventory::default();
        assert_eq!(inv.selected_volume_index(), 0);
        assert_eq!(inv.selected_device_index(), None);
        assert_eq!(inv.selected_path(), PathBuf::from("."));
    }

    #[test]
    fn test_storage_bus_type_mapping() {
        assert_eq!(StorageBusType::from_raw(7), StorageBusType::Usb);
        assert_eq!(StorageBusType::from_raw(11), StorageBusType::Sata);
        assert_eq!(StorageBusType::from_raw(12), StorageBusType::Sd);
        assert_eq!(StorageBusType::from_raw(13), StorageBusType::Mmc);
        assert_eq!(StorageBusType::from_raw(17), StorageBusType::Nvme);
        assert_eq!(StorageBusType::from_raw(99), StorageBusType::Unknown(99));

        assert!(StorageBusType::Usb.is_removable_bus());
        assert!(StorageBusType::Sd.is_removable_bus());
        assert!(StorageBusType::Mmc.is_removable_bus());
        assert!(!StorageBusType::Sata.is_removable_bus());
        assert!(!StorageBusType::Nvme.is_removable_bus());
    }

    #[test]
    fn test_storage_target_safety_check() {
        let mut dev = PhysicalDeviceInfo {
            drive_index: 0,
            path: r"\\.\PhysicalDrive0".to_string(),
            vendor_id: "Internal".to_string(),
            product_id: "SSD".to_string(),
            serial_number: "123".to_string(),
            bus_type: StorageBusType::Nvme,
            total_bytes: 512 * 1024 * 1024 * 1024,
            is_removable: false,
            is_safe_target: false,
            warning: Some("系统主盘已锁定".to_string()),
            mounted_volumes: vec!['C'],
        };

        let target_system = StorageTarget::new(dev.clone(), Vec::new());
        assert!(!target_system.is_safe_for_raw_probe(), "系统主盘 0 必须被拦截");

        dev.drive_index = 1;
        dev.bus_type = StorageBusType::Usb;
        dev.is_removable = true;
        dev.is_safe_target = true;
        dev.mounted_volumes = vec!['E']; // 移除系统盘 C:，赋予合规移动卷 E:
        let target_usb = StorageTarget::new(dev, Vec::new());
        assert!(target_usb.is_safe_for_raw_probe(), "合规移动 U 盘必须允许探测");
    }
}