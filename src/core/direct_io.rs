use std::collections::HashMap;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, SetEndOfFile, SetFilePointerEx, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL,
    FILE_BEGIN, FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

// 标准 Win32 读写访问掩码常量 (不包含与 FILE_FLAG_NO_BUFFERING 冲突的 FILE_APPEND_DATA)
const GENERIC_READ_ACCESS: u32 = 0x8000_0000;
const GENERIC_WRITE_ACCESS: u32 = 0x4000_0000;

// 存储块 I/O 最小抽象接缝 (BlockIo)
// 提供脱离硬件与内核绑定的纯净块读写接口，使核心探针与算法具备脱机可测性
pub trait BlockIo: Send + Sync {
    fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, String>;
    fn write_at(&mut self, buf: &[u8], offset: u64) -> Result<usize, String>;
    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub struct DirectIoFile {
    handle: HANDLE,
    is_write: bool,
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
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => Ok(Self { handle: h, is_write: true }),
            _ => Err(format!(
                "无法以 Direct I/O 写入模式创建文件 {:?}: 错误 {}",
                path,
                std::io::Error::last_os_error()
            )),
        }
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
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => Ok(Self { handle: h, is_write: true }),
            _ => Err(format!(
                "无法打开底层物理设备 {:?}: 错误 {}",
                path,
                std::io::Error::last_os_error()
            )),
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
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.is_invalid() => Ok(Self { handle: h, is_write: false }),
            _ => Err(format!(
                "无法以 Direct I/O 只读模式打开文件 {:?}: 错误 {}",
                path,
                std::io::Error::last_os_error()
            )),
        }
    }

    // 在指定绝对偏移量处写入扇区对齐数据
    pub fn write_at(&mut self, buf: &[u8], offset: u64) -> Result<usize, String> {
        assert_eq!(buf.len() % 512, 0, "写入缓冲区大小必须是 512 字节的整倍数");
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = (offset & 0xFFFF_FFFF) as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

        let mut bytes_written = 0u32;
        let success = unsafe {
            windows::Win32::Storage::FileSystem::WriteFile(
                self.handle,
                Some(buf),
                Some(&mut bytes_written),
                Some(&mut overlapped),
            )
        };

        if success.is_err() {
            // 异步重叠完成等待
            let mut actual_written = 0u32;
            let wait_res = unsafe {
                GetOverlappedResult(self.handle, &overlapped, &mut actual_written, true)
            };
            if wait_res.is_err() {
                return Err(format!("Direct I/O 写入失败: {}", std::io::Error::last_os_error()));
            }
            Ok(actual_written as usize)
        } else {
            Ok(bytes_written as usize)
        }
    }

    // 在指定绝对偏移量处读取扇区对齐数据
    pub fn read_at(&mut self, buf: &mut [u8], offset: u64) -> Result<usize, String> {
        assert_eq!(buf.len() % 512, 0, "读取缓冲区大小必须是 512 字节的整倍数");
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = (offset & 0xFFFF_FFFF) as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

        let mut bytes_read = 0u32;
        let success = unsafe {
            windows::Win32::Storage::FileSystem::ReadFile(
                self.handle,
                Some(buf),
                Some(&mut bytes_read),
                Some(&mut overlapped),
            )
        };

        if success.is_err() {
            let mut actual_read = 0u32;
            let wait_res = unsafe {
                GetOverlappedResult(self.handle, &overlapped, &mut actual_read, true)
            };
            if wait_res.is_err() {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(38) { // ERROR_HANDLE_EOF
                    return Ok(0);
                }
                return Err(format!("Direct I/O 读取失败: {}", err));
            }
            Ok(actual_read as usize)
        } else {
            Ok(bytes_read as usize)
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
        Ok(())
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

        // 真实物理回绕映射
        let mapped_offset = if let Some(real) = self.wrap_around_bytes {
            if real > 0 { offset % real } else { offset }
        } else {
            offset
        };

        if offset >= self.virtual_size_bytes {
            return Ok(0);
        }

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