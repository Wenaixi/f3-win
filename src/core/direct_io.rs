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

pub struct DirectIoFile {
    handle: HANDLE,
    is_write: bool,
}

unsafe impl Send for DirectIoFile {}
unsafe impl Sync for DirectIoFile {}

impl DirectIoFile {
    // 规范化并转为 Windows Wide 字符串，避免 \?\ 与双反斜杠引发的参数错误 87/123
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

impl Drop for DirectIoFile {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE && !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}
