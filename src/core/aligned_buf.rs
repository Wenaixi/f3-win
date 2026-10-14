use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use windows::Win32::System::Memory::{
    VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};

// 4KB 扇区/物理内存页对齐大小
pub const PAGE_ALIGNMENT: usize = 4096;

// 基于 VirtualAlloc 的页对齐内存缓冲区
pub struct AlignedBuffer {
    ptr: NonNull<u8>,
    capacity: usize,
    len: usize,
}

unsafe impl Send for AlignedBuffer {}
unsafe impl Sync for AlignedBuffer {}

impl AlignedBuffer {
    // 分配指定容量的内存（自动向上对齐到 4096 字节）
    pub fn new(capacity: usize) -> Result<Self, String> {
        let aligned_cap = (capacity + PAGE_ALIGNMENT - 1) & !(PAGE_ALIGNMENT - 1);
        unsafe {
            let alloc_ptr = VirtualAlloc(
                None,
                aligned_cap,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            );

            if alloc_ptr.is_null() {
                return Err(format!(
                    "VirtualAlloc 无法分配 {} 字节对齐内存: 错误码 {}",
                    aligned_cap,
                    std::io::Error::last_os_error()
                ));
            }

            Ok(Self {
                ptr: NonNull::new_unchecked(alloc_ptr as *mut u8),
                capacity: aligned_cap,
                len: aligned_cap,
            })
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Deref for AlignedBuffer {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for AlignedBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe {
            let _ = VirtualFree(self.ptr.as_ptr() as *mut _, 0, MEM_RELEASE);
        }
    }
}
