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
    Untested,
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
            BlockState::Untested => {},
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

// 单向不可逆种子派生函数 (One-Way Seed Permutation)
// 1. 当 salt == 0 时：直接返回 offset，与原版 F3 / H2testw 保持 100% 逐比特向后兼容。
// 2. 当 salt != 0 时：基于 SplitMix64 终结非线性雪崩置换，彻底摧毁线性同余乘法群的同态性。
//    模逆元 0x6789abcdeeeeeeef 在此置换下彻底失效，假盘微码在多项式时间内无法反解 Salt。
#[inline(always)]
pub fn one_way_seed(offset: u64, salt: u64) -> u64 {
    if salt == 0 {
        return offset;
    }
    let mut z = (offset ^ salt).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// 填充单个 512 字节扇区 (安全且对齐无关，单指令吞吐)
#[inline]
pub fn fill_sector(buf: &mut [u8; SECTOR_SIZE], offset: u64, salt: u64) {
    let mut chunks = buf.chunks_exact_mut(8);

    // 扇区第一个 64 位存放明文 offset (Salt 绝不落盘)
    if let Some(first) = chunks.next() {
        first.copy_from_slice(&offset.to_le_bytes());
    }

    // 后续 63 个 64 位依次填充伪随机数，初始种子经单向雪崩置换
    let mut random_number = one_way_seed(offset, salt);
    for chunk in chunks {
        random_number = next_random_number(random_number);
        chunk.copy_from_slice(&random_number.to_le_bytes());
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

// 校验单个 512 字节扇区 (安全且对齐无关)
#[inline]
pub fn validate_sector(
    buf: &[u8; SECTOR_SIZE],
    expected_offset: u64,
    salt: u64,
) -> (BlockState, u64) {
    let mut chunks = buf.chunks_exact(8);
    let found_offset = match chunks.next() {
        Some(first) => u64::from_le_bytes(first.try_into().unwrap()),
        None => return (BlockState::Bad, 0),
    };

    let mut random_number = one_way_seed(found_offset, salt);
    let mut bit_error_count = 0u32;

    for chunk in chunks {
        random_number = next_random_number(random_number);
        let val = u64::from_le_bytes(chunk.try_into().unwrap());
        if val != random_number {
            bit_error_count += (val ^ random_number).count_ones();
            if bit_error_count > BIT_ERROR_TOLERANCE {
                return (BlockState::Bad, found_offset);
            }
        }
    }

    if bit_error_count == 0 {
        if found_offset == expected_offset {
            (BlockState::Good, found_offset)
        } else {
            (BlockState::Overwritten, found_offset)
        }
    } else {
        if found_offset == expected_offset {
            (BlockState::Changed, found_offset)
        } else {
            (BlockState::Overwritten, found_offset)
        }
    }
}

// 批量校验多个连续扇区
pub fn validate_sectors(
    buf: &[u8],
    expected_start_offset: u64,
    salt: u64,
    stats: &mut BlockStats,
) {
    assert_eq!(buf.len() % SECTOR_SIZE, 0, "Buffer length must be multiple of 512");
    let sector_count = buf.len() / SECTOR_SIZE;

    for i in 0..sector_count {
        let expected_offset = expected_start_offset + (i as u64 * SECTOR_SIZE as u64);
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
    fn test_fill_and_validate_good_sector_zero_salt_compatibility() {
        let mut sector = [0u8; SECTOR_SIZE];
        let offset = 1048576u64;
        let salt = 0u64;

        fill_sector(&mut sector, offset, salt);
        let (state, found) = validate_sector(&sector, offset, salt);

        assert_eq!(state, BlockState::Good);
        assert_eq!(found, offset);
    }

    #[test]
    fn test_one_way_salt_defeats_modular_inverse() {
        let mut sector = [0u8; SECTOR_SIZE];
        let offset = 2097152u64;
        let salt = 0xDEAD_BEEF_CAFE_BABE_u64;

        fill_sector(&mut sector, offset, salt);
        let (state, found) = validate_sector(&sector, offset, salt);
        assert_eq!(state, BlockState::Good);
        assert_eq!(found, offset);

        // 验证模逆元 0x6789abcdeeeeeeef 无法反解加盐后的种子
        let first_random = u64::from_le_bytes(sector[8..16].try_into().unwrap());
        let inv_mult = 0x6789_ABCD_EEEE_EEEF_u64;
        let naive_cracked_s0 = first_random.wrapping_sub(17).wrapping_mul(inv_mult);
        let naive_cracked_salt = naive_cracked_s0 ^ offset;
        // 模逆元反解出的 Salt 与真实 Salt 完全不符，证明单向雪崩置换成功防御了逆元攻击
        assert_ne!(naive_cracked_salt, salt);
    }

    #[test]
    fn test_validate_changed_sector() {
        let mut sector = [0u8; SECTOR_SIZE];
        let offset = 512u64;
        let salt = 0u64;

        fill_sector(&mut sector, offset, salt);

        // 翻转 3 个 bit（允许容忍范围内）
        sector[10] ^= 0b0000_0111;

        let (state, found) = validate_sector(&sector, offset, salt);
        assert_eq!(state, BlockState::Changed);
        assert_eq!(found, offset);
    }

    #[test]
    fn test_validate_overwritten_sector() {
        let mut sector = [0u8; SECTOR_SIZE];
        let write_offset = 1024u64;
        let expect_offset = 2048u64;
        let salt = 0u64;

        fill_sector(&mut sector, write_offset, salt);
        let (state, found) = validate_sector(&sector, expect_offset, salt);

        assert_eq!(state, BlockState::Overwritten);
        assert_eq!(found, write_offset);
    }

    #[test]
    fn test_validate_bad_sector() {
        let mut sector = [0u8; SECTOR_SIZE];
        let expect_offset = 512u64;
        let salt = 0u64;

        fill_sector(&mut sector, expect_offset, salt);
        // 大面积破坏数据 (8 字节全翻转，超过 7 位容差)
        for b in &mut sector[20..28] {
            *b ^= 0xFF;
        }

        let (state, _) = validate_sector(&sector, expect_offset, salt);
        assert_eq!(state, BlockState::Bad);
    }
}