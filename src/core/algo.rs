// 核心扇区大小与常量定义
pub const SECTOR_ORDER: u32 = 9;
pub const SECTOR_SIZE: usize = 1 << SECTOR_ORDER; // 512 字节
pub const NUM_INT64_PER_SECTOR: usize = SECTOR_SIZE / 8; // 64 个 u64
pub const BIT_ERROR_TOLERANCE: u32 = 7;

// 原版 F3 线性同余发生器乘数与增量常量
pub const LCG_MULTIPLIER: u64 = 4294967311;
pub const LCG_INCREMENT: u64 = 17;

// 扇区健康状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    Good,
    Bad,
    Changed,
    Overwritten,
}

// 统计结构体
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BlockStats {
    pub good: u64,
    pub bad: u64,
    pub changed: u64,
    pub overwritten: u64,
}

impl BlockStats {
    pub fn update(&mut self, state: BlockState) {
        match state {
            BlockState::Good => self.good += 1,
            BlockState::Bad => self.bad += 1,
            BlockState::Changed => self.changed += 1,
            BlockState::Overwritten => self.overwritten += 1,
        }
    }
}

// 单步线性同余随机数计算
#[inline(always)]
pub fn next_random_number(random_number: u64) -> u64 {
    random_number.wrapping_mul(LCG_MULTIPLIER).wrapping_add(LCG_INCREMENT)
}

// 填充单个 512 字节扇区
#[inline]
pub fn fill_sector(buf: &mut [u8; SECTOR_SIZE], offset: u64, salt: u64) {
    let int64_array = unsafe {
        std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u64, NUM_INT64_PER_SECTOR)
    };

    // 扇区第一个 64 位存放 offset
    int64_array[0] = offset;

    // 后续 63 个 64 位依次填充伪随机数
    let mut random_number = offset ^ salt;
    for slot in int64_array.iter_mut().skip(1) {
        random_number = next_random_number(random_number);
        *slot = random_number;
    }
}

// 批量填充多个连续扇区（支持自动步进 offset）
pub fn fill_sectors(buf: &mut [u8], start_offset: u64, salt: u64) {
    assert_eq!(buf.len() % SECTOR_SIZE, 0, "Buffer length must be multiple of 512");
    let sector_count = buf.len() / SECTOR_SIZE;

    for i in 0..sector_count {
        let sector_offset = start_offset + (i as u64 * SECTOR_SIZE as u64);
        let slice = &mut buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE];
        let sector_buf: &mut [u8; SECTOR_SIZE] = slice.try_into().expect("Slice matches sector size");
        fill_sector(sector_buf, sector_offset, salt);
    }
}

// 校验单个 512 字节扇区
#[inline]
pub fn validate_sector(
    buf: &[u8; SECTOR_SIZE],
    expected_offset: u64,
    salt: u64,
) -> (BlockState, u64) {
    let int64_array = unsafe {
        std::slice::from_raw_parts(buf.as_ptr() as *const u64, NUM_INT64_PER_SECTOR)
    };

    let found_offset = int64_array[0];
    let mut random_number = found_offset ^ salt;
    let mut bit_error_count = 0u32;

    // 优化：先尝试快速整字比对，遇到不一致时才计算 popcount
    for &val in int64_array.iter().skip(1) {
        random_number = next_random_number(random_number);
        if val != random_number {
            bit_error_count += (val ^ random_number).count_ones();
            if bit_error_count > BIT_ERROR_TOLERANCE {
                break;
            }
        }
    }

    let state = if expected_offset == found_offset {
        if bit_error_count == 0 {
            BlockState::Good
        } else if bit_error_count <= BIT_ERROR_TOLERANCE {
            BlockState::Changed
        } else {
            BlockState::Bad
        }
    } else if bit_error_count <= BIT_ERROR_TOLERANCE {
        BlockState::Overwritten
    } else {
        BlockState::Bad
    };

    (state, found_offset)
}

// 批量校验连续扇区并更新统计
pub fn validate_sectors(
    buf: &[u8],
    start_expected_offset: u64,
    salt: u64,
    stats: &mut BlockStats,
) {
    assert_eq!(buf.len() % SECTOR_SIZE, 0, "Buffer length must be multiple of 512");
    let sector_count = buf.len() / SECTOR_SIZE;

    for i in 0..sector_count {
        let expected_offset = start_expected_offset + (i as u64 * SECTOR_SIZE as u64);
        let slice = &buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE];
        let sector_buf: &[u8; SECTOR_SIZE] = slice.try_into().expect("Slice matches sector size");
        let (state, _) = validate_sector(sector_buf, expected_offset, salt);
        stats.update(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lcg_sequence_consistency() {
        let mut r = 0u64;
        r = next_random_number(r);
        assert_eq!(r, 17);
        r = next_random_number(r);
        assert_eq!(r, 17 * 4294967311 + 17);
    }

    #[test]
    fn test_fill_and_validate_good_sector() {
        let mut sector = [0u8; SECTOR_SIZE];
        let offset = 1048576u64;
        let salt = 0u64;

        fill_sector(&mut sector, offset, salt);
        let (state, found) = validate_sector(&sector, offset, salt);

        assert_eq!(state, BlockState::Good);
        assert_eq!(found, offset);
    }

    #[test]
    fn test_validate_changed_sector() {
        let mut sector = [0u8; SECTOR_SIZE];
        let offset = 512u64;
        let salt = 0u64;

        fill_sector(&mut sector, offset, salt);
        // 翻转 2 位
        sector[10] ^= 0b00000011;

        let (state, found) = validate_sector(&sector, offset, salt);
        assert_eq!(state, BlockState::Changed);
        assert_eq!(found, offset);
    }

    #[test]
    fn test_validate_overwritten_sector() {
        let mut sector = [0u8; SECTOR_SIZE];
        let write_offset = 2048u64;
        let expect_offset = 1024u64;
        let salt = 0u64;

        fill_sector(&mut sector, write_offset, salt);
        let (state, found) = validate_sector(&sector, expect_offset, salt);

        assert_eq!(state, BlockState::Overwritten);
        assert_eq!(found, write_offset);
    }

    #[test]
    fn test_validate_bad_sector() {
        let sector = [0xAAu8; SECTOR_SIZE]; // 填满垃圾数据
        let expect_offset = 0u64;
        let salt = 0u64;

        let (state, _) = validate_sector(&sector, expect_offset, salt);
        assert_eq!(state, BlockState::Bad);
    }
}
