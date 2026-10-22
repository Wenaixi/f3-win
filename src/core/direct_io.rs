use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, SetEndOfFile, SetFilePointerEx, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL,
    FILE_BEGIN, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
use windows::Win32::System::Ioctl::{DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX};

// 标准 Win32 读写访问掩码常量 (不包含与 FILE_FLAG_NO_BUFFERING 冲突的 FILE_APPEND_DATA)
const GENERIC_READ_ACCESS: u32 = 0x8000_0000;
const GENERIC_WRITE_ACCESS: u32 = 0x4000_0000;

/// 解析 Win32 原始错误码 (从 HRESULT 中提取 Facility 7 的 low-16 位，或直接保留原始代码)
#[inline]
pub fn parse_win32_code(hr: i32) -> u32 {
    let u = hr as u32;
    if (u & 0xFFFF_0000) == 0x8007_0000 {
        u & 0xFFFF
    } else {
        u
    }
}

// 存储块 I/O 最小抽象接缝 (BlockIo)
// 提供脱离硬件与内核绑定的纯净块读写接口，使核心探针与算法具备脱机可测性
pub trait BlockIo: Send + Sync {
    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, String>;
    fn write_at(&mut self, buf: &[u8], offset: u64) -> Result<usize, String>;
    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn sector_size(&self) -> usize {
        512
    }
}

pub struct DirectIoFile {
    handle: HANDLE,
    is_write: bool,
    sector_size: usize,
}

unsafe impl Send for DirectIoFile {}
unsafe impl Sync for DirectIoFile {}

impl DirectIoFile {
    // 规范化并转为 Windows Wide 字符串，避免 \\?\ 与双反斜杠引发的参数错误 87/123
    fn to_wide_path(path: &Path) -> Vec<u16> {
        let path_str = path.to_string_lossy();
        let cleaned = path_str.replace('/', "\\");
        
        let mut normalized = String::with_capacity(cleaned.len());
        let mut prev_is_slash = false;

        for (idx, ch) in cleaned.chars().enumerate() {
            if ch == '\\' {
                // 允许开头的 \\ (如果是 UNC 共享路径)
                if idx == 0 || idx == 1 || !prev_is_slash {
                    normalized.push(ch);
                    prev_is_slash = true;
                }
            } else {
                normalized.push(ch);
                prev_is_slash = false;
            }
        }

        normalized.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // 以 Direct I/O 无缓冲模式创建或覆盖写入文件 (使用 GENERIC_WRITE_ACCESS 0x40000000)
    pub fn create_write(path: &Path) -> Result<Self, String> {
        let wide = Self::to_wide_path(path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_WRITE_ACCESS,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
                HANDLE::default(),
            )
        };

        match handle {
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => Ok(Self { handle: h, is_write: true, sector_size: 512 }),
            _ => {
                let os_err = std::io::Error::last_os_error();
                let code = os_err.raw_os_error().map(|c| c as u32).unwrap_or(0);
                let friendly = match code {
                    19 => " (目标磁盘处于写保护状态，请检查物理写保护开关或只读属性)",
                    1167 | 1117 | 21 => " (设备未连接或发生严重硬件掉盘故障)",
                    _ => "",
                };
                Err(format!(
                    "无法以 Direct I/O 写入模式创建文件 {:?}: 错误 [Win32: {}] {}{}",
                    path,
                    code,
                    os_err,
                    friendly
                ))
            }
        }
    }

    // 探测底层物理设备的真实物理扇区大小 (自动识别 512 字节或 4096 字节原生 4Kn 介质)
    fn query_raw_device_sector_size(handle: HANDLE) -> usize {
        let mut geom = DISK_GEOMETRY_EX::default();
        let mut bytes_returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
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

        if ok.is_ok() {
            let bps = geom.Geometry.BytesPerSector as usize;
            if bps >= 512 && bps.is_power_of_two() {
                return bps;
            }
        }
        512
    }

    // 专门用于打开底层物理磁盘 (\\.\PhysicalDriveX) 或逻辑卷 (\\.\X:)
    // 必须使用 OPEN_EXISTING，且同时赋予 GENERIC_READ 与 GENERIC_WRITE 访问权限
    pub fn open_raw_device(path: &Path) -> Result<Self, String> {
        let wide = Self::to_wide_path(path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ_ACCESS | GENERIC_WRITE_ACCESS,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING, // 物理设备必须使用 OPEN_EXISTING，严禁 CREATE_ALWAYS
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
                HANDLE::default(),
            )
        };

        match handle {
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => {
                let sector_size = Self::query_raw_device_sector_size(h);
                Ok(Self { handle: h, is_write: true, sector_size })
            }
            _ => {
                let os_err = std::io::Error::last_os_error();
                let code = os_err.raw_os_error().map(|c| c as u32).unwrap_or(0);
                let friendly = match code {
                    19 => " (目标物理设备处于写保护状态，请检查硬件写保护拨码开关)",
                    1167 | 1117 | 21 => " (设备未连接或发生严重硬件掉盘故障)",
                    _ => "",
                };
                Err(format!(
                    "无法打开底层物理设备 {:?}: 错误 [Win32: {}] {}{}",
                    path,
                    code,
                    os_err,
                    friendly
                ))
            }
        }
    }

    // 以 Direct I/O 无缓冲模式只读打开文件 (使用 GENERIC_READ_ACCESS 0x80000000)
    pub fn open_read(path: &Path) -> Result<Self, String> {
        let wide = Self::to_wide_path(path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ_ACCESS,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING,
                HANDLE::default(),
            )
        };

        match handle {
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => Ok(Self { handle: h, is_write: false, sector_size: 512 }),
            _ => Err(format!(
                "无法以 Direct I/O 只读模式打开文件 {:?}: 错误 {}",
                path,
                std::io::Error::last_os_error()
            )),
        }
    }

    // 设置物理扇区对齐大小 (支持 512 字节与 4096 字节原生 4Kn 介质)
    pub fn set_sector_size(&mut self, size: usize) {
        if size >= 512 && size.is_power_of_two() {
            self.sector_size = size;
        }
    }

    // 获取当前物理扇区对齐大小
    pub fn sector_size(&self) -> usize {
        self.sector_size
    }

    // 强制下发硬件级缓存穿透冲刷 (向主控下发 SCSI SYNCHRONIZE CACHE / NVMe Flush)
    pub fn flush(&mut self) -> Result<(), String> {
        if self.is_write {
            let res = unsafe { FlushFileBuffers(self.handle) };
            if res.is_err() {
                return Err(format!(
                    "FlushFileBuffers 硬件穿透冲刷失败: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(())
    }

    // 在指定绝对偏移量处写入扇区对齐数据
    pub fn write_at(&mut self, buf: &[u8], offset: u64) -> Result<usize, String> {
        assert_eq!(
            buf.len() % self.sector_size,
            0,
            "写入缓冲区大小必须是物理扇区大小 ({}) 的整倍数",
            self.sector_size
        );
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = (offset & 0xFFFF_FFFF) as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

        let mut bytes_written = 0u32;
        let res = unsafe {
            windows::Win32::Storage::FileSystem::WriteFile(
                self.handle,
                Some(buf),
                Some(&mut bytes_written),
                Some(&mut overlapped),
            )
        };

        match res {
            Ok(_) => {
                // 驱动可能短写甚至写出 0 字节，此处必须校验足量写入：
                // 短写若被当成正常进度，会掩盖设备异常；0 字节会让 session 的
                // 写入循环永无进展。统一在此处暴露为错误，由调用方决定是否重试或中止。
                let written = bytes_written as usize;
                if written != buf.len() {
                    return Err(format!(
                        "Direct I/O 写入不足: 期望 {} 字节, 实际 {} 字节 (偏移 {})",
                        buf.len(),
                        written,
                        offset
                    ));
                }
                Ok(written)
            }
            Err(e) => {
                let code = parse_win32_code(e.code().0);
                let friendly = match code {
                    19 => " (介质受写入保护，请检查物理写保护开关或只读属性)",
                    1167 => " (设备已断开连接/突发硬件拔盘)",
                    1117 => " (硬件 I/O 设备错误/主控无响应)",
                    21 => " (设备未就绪/突发掉盘)",
                    23 => " (数据错误 CRC/物理坏块)",
                    112 | 39 => " (磁盘空间已满)",
                    _ => "",
                };
                Err(format!(
                    "Direct I/O 写入失败 [Win32: {}]: {}{}",
                    code,
                    e.message(),
                    friendly
                ))
            }
        }
    }

    // 在指定绝对偏移量处读取扇区对齐数据
    pub fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, String> {
        assert_eq!(
            buf.len() % self.sector_size,
            0,
            "读取缓冲区大小必须是物理扇区大小 ({}) 的整倍数",
            self.sector_size
        );
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = (offset & 0xFFFF_FFFF) as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

        let mut bytes_read = 0u32;
        let res = unsafe {
            windows::Win32::Storage::FileSystem::ReadFile(
                self.handle,
                Some(buf),
                Some(&mut bytes_read),
                Some(&mut overlapped),
            )
        };

        match res {
            Ok(_) => Ok(bytes_read as usize),
            Err(e) => {
                let code = parse_win32_code(e.code().0);
                // Win32 ERROR_HANDLE_EOF (38) 是正常的读取文件末尾标记
                if code == 38 {
                    Ok(0)
                } else {
                    let friendly = match code {
                        1167 => " (设备已断开连接/突发硬件拔盘)",
                        1117 => " (硬件 I/O 设备错误/主控无响应)",
                        21 => " (设备未就绪/突发掉盘)",
                        23 => " (数据错误 CRC/物理坏块)",
                        _ => "",
                    };
                    Err(format!(
                        "Direct I/O 读取失败 [Win32: {}]: {}{}",
                        code,
                        e.message(),
                        friendly
                    ))
                }
            }
        }
    }

    // 截断文件到指定精确长度（用于清除文件末尾对齐多写出的 Padding）
    pub fn truncate_to(&mut self, length: u64) -> Result<(), String> {
        unsafe {
            let mut new_pos = 0i64;
            let pos_res = SetFilePointerEx(self.handle, length as i64, Some(&mut new_pos), FILE_BEGIN);
            if pos_res.is_err() {
                return Err(format!("SetFilePointerEx 失败: {}", std::io::Error::last_os_error()));
            }

            let eof_res = SetEndOfFile(self.handle);
            if eof_res.is_err() {
                return Err(format!("SetEndOfFile 失败: {}", std::io::Error::last_os_error()));
            }
            Ok(())
        }
    }
}

// 生产驱动接入 BlockIo 最小接缝
impl BlockIo for DirectIoFile {
    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, String> {
        self.read_at(buf, offset)
    }

    fn write_at(&mut self, buf: &[u8], offset: u64) -> Result<usize, String> {
        self.write_at(buf, offset)
    }

    fn flush(&mut self) -> Result<(), String> {
        self.flush()
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }
}

impl Drop for DirectIoFile {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE && !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

// 用于脱机单测与 CI 验证的内存块设备测试桩
// 支持模拟正常物理存储或带有容量虚标（地址回绕）的扩容闪存
#[derive(Default)]
pub struct MemoryBlockDevice {
    sectors: HashMap<u64, Vec<u8>>,
    virtual_size_bytes: u64,
    wrap_around_bytes: Option<u64>,
    bit_bucket_bytes: Option<u64>,
}

impl MemoryBlockDevice {
    pub fn new(virtual_size_bytes: u64) -> Self {
        Self {
            sectors: HashMap::new(),
            virtual_size_bytes,
            wrap_around_bytes: None,
            bit_bucket_bytes: None,
        }
    }

    pub fn with_fake_wrap(virtual_size_bytes: u64, real_size_bytes: u64) -> Self {
        Self {
            sectors: HashMap::new(),
            virtual_size_bytes,
            wrap_around_bytes: Some(real_size_bytes),
            bit_bucket_bytes: None,
        }
    }

    pub fn with_fake_bit_bucket(virtual_size_bytes: u64, real_size_bytes: u64) -> Self {
        Self {
            sectors: HashMap::new(),
            virtual_size_bytes,
            wrap_around_bytes: None,
            bit_bucket_bytes: Some(real_size_bytes),
        }
    }
}

impl BlockIo for MemoryBlockDevice {
    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, String> {
        if offset >= self.virtual_size_bytes {
            return Ok(0);
        }

        // 位黑洞模式：超出真实容量直接返回全 0 (模拟物理颗粒缺失)
        if let Some(real) = self.bit_bucket_bytes {
            if offset >= real {
                buf.fill(0);
                return Ok(buf.len());
            }
        }

        // 真实物理回绕映射
        let mapped_offset = if let Some(real) = self.wrap_around_bytes {
            if real > 0 { offset % real } else { offset }
        } else {
            offset
        };

        let sector_idx = mapped_offset / 512;
        let sector_offset = (mapped_offset % 512) as usize;
        let to_read = buf.len().min((self.virtual_size_bytes - offset) as usize);

        for (i, byte) in buf[..to_read].iter_mut().enumerate() {
            let curr_sec = sector_idx + (sector_offset + i) as u64 / 512;
            let byte_in_sec = (sector_offset + i) % 512;
            if let Some(sec_data) = self.sectors.get(&curr_sec) {
                *byte = sec_data[byte_in_sec];
            } else {
                *byte = 0;
            }
        }
        Ok(to_read)
    }

    fn write_at(&mut self, buf: &[u8], offset: u64) -> Result<usize, String> {
        if offset >= self.virtual_size_bytes {
            return Err("写入超出虚拟设备边界".to_string());
        }

        // 位黑洞模式：超出真实容量的写入直接静默丢弃 (模拟 FTL 不刷入物理颗粒)
        if let Some(real) = self.bit_bucket_bytes {
            if offset >= real {
                return Ok(buf.len());
            }
        }

        let mapped_offset = if let Some(real) = self.wrap_around_bytes {
            if real > 0 { offset % real } else { offset }
        } else {
            offset
        };

        let sector_idx = mapped_offset / 512;
        let sector_offset = (mapped_offset % 512) as usize;

        for (i, &byte) in buf.iter().enumerate() {
            let curr_sec = sector_idx + (sector_offset + i) as u64 / 512;
            let byte_in_sec = (sector_offset + i) % 512;
            let sec_data = self.sectors.entry(curr_sec).or_insert_with(|| vec![0u8; 512]);
            sec_data[byte_in_sec] = byte;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_block_device_read_write() {
        let mut dev = MemoryBlockDevice::new(4 * 1024 * 1024); // 4MB 虚拟设备，容纳 1MB 探针扇区
        let test_data = vec![0xABu8; 512];
        let write_len = dev.write_at(&test_data, 2048 * 512).unwrap();
        assert_eq!(write_len, 512);

        let mut read_buf = vec![0u8; 512];
        let read_len = dev.read_at(&mut read_buf, 2048 * 512).unwrap();
        assert_eq!(read_len, 512);
        assert_eq!(read_buf, test_data);
    }

    #[test]
    fn test_parse_win32_code_hresult_and_raw() {
        assert_eq!(parse_win32_code(0x80070070u32 as i32), 112); // ERROR_DISK_FULL
        assert_eq!(parse_win32_code(0x80070027u32 as i32), 39);  // ERROR_HANDLE_DISK_FULL
        assert_eq!(parse_win32_code(0x80070013u32 as i32), 19);  // ERROR_WRITE_PROTECT
        assert_eq!(parse_win32_code(0x8007048Fu32 as i32), 1167); // ERROR_DEVICE_NOT_CONNECTED
        assert_eq!(parse_win32_code(0x8007045Du32 as i32), 1117); // ERROR_IO_DEVICE
        assert_eq!(parse_win32_code(0x80070015u32 as i32), 21);  // ERROR_NOT_READY
        assert_eq!(parse_win32_code(0x80070017u32 as i32), 23);  // ERROR_CRC
        assert_eq!(parse_win32_code(112), 112);
        assert_eq!(parse_win32_code(19), 19);
    }

    #[test]
    fn test_memory_block_device_fake_wrap_around() {
        // 虚拟 64GB 磁盘，真实物理容量仅 4GB (4 * 1024 * 1024 * 1024)
        let virtual_size = 64 * 1024 * 1024 * 1024u64;
        let real_size = 4 * 1024 * 1024 * 1024u64;
        let mut dev = MemoryBlockDevice::with_fake_wrap(virtual_size, real_size);

        // 写入物理基准扇区 (例如 1MB 偏移处)
        let mut base_data = vec![0u8; 512];
        base_data[0] = 0x55;
        base_data[511] = 0xAA;
        dev.write_at(&base_data, 1024 * 1024).unwrap();

        // 探测读取高位偏移量处：基准偏移量 + 真实容量 (发生地址回绕，应当读取到 base_data)
        let mut wrapped_buf = vec![0u8; 512];
        dev.read_at(&mut wrapped_buf, 1024 * 1024 + real_size).unwrap();
        assert_eq!(wrapped_buf, base_data);

        // 高位偏移量 + 2 * real_size 同样回绕
        let mut wrapped_buf2 = vec![0u8; 512];
        dev.read_at(&mut wrapped_buf2, 1024 * 1024 + 2 * real_size).unwrap();
        assert_eq!(wrapped_buf2, base_data);
    }
}