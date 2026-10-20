# 审计 02：probe.rs 对照上游 libprobe.c

审计对象：D:/newC/stick3/f3-win/src/core/probe.rs（671 行）
上游基准：D:/newC/stick3/f3-win/.upstream_f3/src/libprobe.c（960 行）、libutils.c
方法：静态逐行阅读，未编译、未运行测试、未修改任何源码。所有 LBA 推演用独立脚本按代码逻辑重算。

---

## 0 上游行为与常量（必查项 a 的答案）

### overwrite 的判定：上游没有 overwrite 这个词，等价概念是 bs_overwritten

上游 libutils.c:258-296 的 validate_buffer_with_block 返回五种块状态：

- libutils.c:284-290：读回的偏移等于期望偏移时，按比特错数分流。0 比特错为 bs_good，小于等于 7 比特错为 bs_changed，否则为 bs_bad。
- libutils.c:292-293：读回的偏移不等于期望偏移，但内容本身是合法块，返回 bs_overwritten。
- libutils.c:295：其它为 bs_bad。

关键点：bs_overwritten 是"被别的块覆盖了"的独立语义，libprobe.c:259-282 的 cb_bad_block_info 会据此报出"Block X overwrites block Y"（libprobe.c:274-275）或"comes from the cache block"（libprobe.c:271-272）。本项目 probe.rs:50-57 的 ProbeTrap 只有 None / BitBucket / WrapAround 三个值，没有 overwritten 这一档。

### overwhelm_cache 的实际行为

libprobe.c:135-143：

    static int overwhelm_cache(struct device *dev, struct rdwr_info *rwi, ...)
    {
        if (rwi->cache_size_block == 0)
            return false;
        cb(indent, "Overwhelming cache\n");
        return write_blocks(dev, rwi->cache_pos,
            rwi->cache_pos + rwi->cache_size_block - 1, rwi, cb, indent);
    }

冲刷量与冲刷位置都不是常量，是 find_cache_size 实测出来的（libprobe.c:568-637）：cache_pos 是首次判定"疑似缓存"的位置，cache_size_block 是实测缓存块数。libprobe.c:757-764 的注释明确说明，这部分地址在 find_cache_size 阶段已经被写过一次，safe_device 会去重，所以重复冲刷几乎零额外代价。

### check_first / check_last 的对应物

上游没有名为 check_first/check_last 的函数。语义最接近的是 libprobe.c:205-249 的 find_first_x_block，它对一组给定块位置按顺序读回、返回第一个不属于 bs_set 的块的下标，其中 libprobe.c:235-242 是"找到即返回"，libprobe.c:246-248 是"一个都没找到时把下标置为 n_blocks"。

### 常量全表（值与原行号）

| 常量 | 值 | 位置 |
| --- | --- | --- |
| SECTOR_ORDER | 9（512 字节） | libutils.h:8 |
| MEGABYTE_ORDER | 20 | libutils.h:10 |
| GIGABYTE_ORDER | 30 | libutils.h:11 |
| MAX_CACHE_SIZE_BYTE | GIGABYTE_SIZE = 2^30 = 1 GiB | libprobe.c:565-566 |
| SAMPLING_MIN | 8 | libprobe.c:513 |
| SAMPLING_MAX | 1024 | libprobe.c:514 |
| probabilistic_test 采样数 n_samples | 64 | libprobe.c:413 |
| bit_error_tolerance | 7 | libutils.c:266 |
| left_pos 初始值 | (1MB >> block_order) - 1 = LBA 2047 | libprobe.c:875 |
| right_pos 初始值 | dev_size_byte >> block_order（盘外第一个块） | libprobe.c:880 |
| find_cache_size 的 write_target 起点/终点 | 1 块 / MAX_CACHE_SIZE_BYTE >> block_order | libprobe.c:574-575、624-626 |
| find_wrap 的 good_block | left_pos + 1 = LBA 2048 | libprobe.c:643 |
| salt 来源 | srand(time(NULL)) + uint64_rand() | libprobe.c:899-901 |

对照本项目的常量：CACHE_FLUSH_BLOCK_SECTORS = 128（64KB，probe.rs:44）、CACHE_FLUSH_MIN_SECTORS = 2048（1MB，probe.rs:47）、CACHE_FLUSH_MAX_SECTORS = 16384（8MB，probe.rs:48）、BISECTION_TOLERANCE_SECTORS = 2048（1MB，probe.rs:41）、MIN_SAFE_SECTORS = 8192（4MB，probe.rs:11）、flush_depth = (total_sectors/16).clamp(2048, 16384)（probe.rs:326）。

上游最关键的差异：上游冲刷量是**实测**的缓存大小，可以到 1 GiB；本项目上限 8 MiB，且按**标称容量**而非物理可用容量计算。

---

## 1 发现清单

### P-01（Critical）Phase 1 的 safe_left 会被缓存排空窗口的污染推过真实物理边界，二分随后在越界区间内收敛，8GB 颗粒可被报成 192GB

**现象**
probe.rs:250-252 把"基准探针失配"一律判为 WrapAround。但基准探针失配有两种成因：真实回绕覆写，以及本扇区自己的缓存排空窗口在回绕介质上跨过了回绕点、把基准扇区覆盖掉。代码不区分这两者。被污染的点返回 Some(WrapAround)，Phase 1 在 probe.rs:416-421 把它当作"第一个失配点"并 break，并把 safe_left 留在**上一个通过点**——而那个"通过点"往往本身已经越界，只是它的排空窗口没覆盖到基准扇区，因此侥幸返回 None。

**证据**
- probe.rs:200-208：scratch_window 只按**逻辑 LBA** 计算窗口下界（lower_bound = 4224）与上界（sector），完全不知道真实物理容量。
- probe.rs:407-421：Phase 1 通过即 safe_left = test_sec（probe.rs:410），失配即 suspected_right = test_sec 并 break（probe.rs:418-420）。
- probe.rs:250-252：基准失配优先于本扇区失配，一律 WrapAround。
- probe.rs:485-531：二分区间 [bisection_left, bisection_right] 由上述 safe_left 与 suspected_right 决定，probe.rs:488 的循环条件只要区间宽于 2048 扇区就继续。

**上游差异**
libprobe.c:519-553 的 sampling_probe 不做"上一点通过就当左锚点"的推断。它的每次迭代都在 (left_pos, *pright_pos) 开区间内**重新随机采 n 个块**（libprobe.c:466-482），一旦其中任何一个坏就直接把 *pright_pos 收缩到那个块（libprobe.c:492-497），因此坏块边界只会单调左移，不存在"某个点侥幸通过就把左锚点推过去"的路径。更根本的是 libprobe.c:639-732 的 find_wrap：它先用代数手段（写 good_block，再读 2^m + good_block）把回绕上界钳死（libprobe.c:708-730），**在采任何样本之前**就已经知道回绕地址的幂次，本项目完全没有这一步。

**LBA 数值推演（真实 33 MiB，标称 64 GiB）**
真实物理容量 R = 67,584 扇区，标称 total_sectors = 134,217,728 扇区，flush_depth = 16,384 扇区（8 MiB）。

Phase 1 依次：

| test_sec | 自身物理落点 t%R | 排空窗口 [start, t) | 窗口是否覆盖物理 LBA 2048 | 判定 |
| --- | --- | --- | --- | --- |
| 6144 | 6144 | [4224, 6144) | 否 | None，safe_left=6144 |
| 10240 | 10240 | [4224, 10240) | 否 | None，safe_left=10240 |
| 18432 | 18432 | [4224, 18432) | 否 | None，safe_left=18432 |
| 34816 | 34816 | [18432, 34816) | 否 | None，safe_left=34816 |
| 67584 | 0 | [51200, 67584) | 否 | None，safe_left=67584 |
| 133120 | 65536 | [116736, 133120) | 否 | None，safe_left=133120 |
| 264192 | 61440 | [247808, 264192) | 否 | None，safe_left=264192 |
| 526336 | 53248 | [509952, 526336) | 否 | None，safe_left=526336 |
| 1050624 | 36864 | [1034240, 1050624) | 否 | None，safe_left=1050624 |
| 2099200 | 4096 | [2082816, 2099200) | **是** | WrapAround，break |

每个 test_sec 都远大于真实边界 67,584（只有前四个在边界内），但因为 8 MiB 的窗口在 R = 67,584 扇区的盘上会**折叠多次**，除最后一个外每次都恰好没有折到物理 2048/4096 这两个基准扇区，于是全部返回 None。safe_left 被推到 1,050,624 扇区，**已经超过真实边界 67,584 达 983,040 扇区（480 MiB）**。

随后二分在 [1050624, 2099200] 区间运行，该区间整体位于越界区，9 步后收敛到 bisection_left = 1,622,016 扇区 = 792 MiB。probe.rs:531 令 final_real_sectors = 1,622,016，最终 real_size = 792 MiB，而真实颗粒 33 MiB，**高估 24 倍**。

同类构造的扫描结果（标称 64 GiB，真实容量取 4~200 MiB）：17、18、19、20、21、22、23、33、34、35、36、37 MiB 等真实容量全部落入该构造，其中真实 37 MiB 时 safe_left 被推到 67,110,912 扇区，报告容量 65,526 MiB，误差 65,489 MiB。

**攻击者如何利用**
扩容盘固件只要让真实颗粒选一个**非 2 的幂、且使各采样点折叠后避开 2048/4096 的容量**（上面那批容量都是自然存在的小颗粒规格），就能让 Phase 1 的基准探针检测全程沉默；等到第一次命中时，safe_left 早已被推过真实边界，二分只能在这个错误区间内收尾，最终报告一个远远偏大的"真实容量"。用户按这个数值重建分区会直接丢数据，而程序会显示"检测到扩容盘，真实物理可用约 792 MB"这类自相矛盾的结论。相对地，若固件采用大容量（真实 8 GiB、标称 64 GiB），本实现的推演结果是正确的：t = 16,779,264 时物理落点恰为 2048，判 WrapAround，二分 12 步收敛到 left = 16,777,216，误差 0。也就是说**本实现对标准 2 的幂容量是准的，对非 2 的幂小颗粒会系统性高估**。

**修复方向**
把"缓存排空窗口"与"基准探针检测"解耦：排空窗口的写入区必须显式排除 LBA 2048/4096（按物理映射判断，或者干脆把窗口挪到远离基准的固定区段）；同时 Phase 1 每个通过点在记录 safe_left 之前，必须复核基准探针在**该点写入前**就已完好，用"写前基线"而非"写后快照"来判定回绕。

---

### P-02（High）判定优先级把"窗口污染"和"真实回绕"合并，任何一次基准失配都会被贴上 WrapAround 标签，结论类型不可信

**现象**
probe.rs:250-252 只做 !base_ok || !aux_ok 就返回 WrapAround，probe.rs:253-255 才看本扇区探针。而 base_ok / aux_ok 的计算（probe.rs:242-247）把**读失败**与**内容不符**合并成同一个 false，也无法区分"基准被别的探针覆盖"与"基准被冲刷白噪声覆盖"。

**证据**
- probe.rs:242-243：base_ok = dev.read_at(...).is_ok() && verify_probe(...)。读 I/O 错误与内容失配的结果相同。
- probe.rs:246-247：aux_ok 同上。
- probe.rs:250-255：判定顺序固定，基准优先级高于本扇区。
- probe.rs:543-553：trap 直接决定给用户看的文案，"物理回绕覆写"与"位黑洞丢弃"是两种完全不同的故障，给用户的处置建议也不同。

**上游差异**
上游把这两件事拆成两个独立信号：libprobe.c:725-730 只在 x_blocks[i] 读回的内容**就是 good_block 的内容**（bs_good 或 bs_changed，libprobe.c:663）时才认定回绕，并据此设置 *pright_pos；而单纯的读错误由 libprobe.c:148-155 的 read_block 单独报 "I/O ERROR" 并中止探测，不进入任何坏块结论。libutils.c:292-293 的 bs_overwritten 又进一步把"被覆盖"与"坏块"分开。

**攻击者如何利用**
固件只需在探测过程中让任意一个基准扇区偶发返回一次读错误（USB 链路抖动、固件主动断开重连都可以做到），本实现就会给出"物理回绕覆写"的确定性结论。反过来，真实回绕若恰好让基准被采样点自身的探针覆盖，固件也可以伪装成"故障"来干扰结论。用户无法从报告里区分"这是假盘"还是"这是探测失败"。

**修复方向**
三态化：基准读失败 → 中止探测并报设备错误；基准内容不符且与本次采样写入内容一致 → WrapAround；基准内容不符且是其它数据 → 单独一类"基准被覆盖"。

---

### P-03（High）探针扇区前 8 字节是明文扇区号，固件识别成本几乎为零，本项目并不比上游强

**现象**
probe.rs:105-123 的 create_probe_block 把 sector_num 以小端原样写进 buf[0..8]（probe.rs:114），紧接着 buf[8..16] 是 check_val（probe.rs:115），其余 496 字节是高熵流。固件只要读前 8 字节并检查它是否等于本次写入的 LBA，就能以单次比较识别出探针扇区。

**证据**
- probe.rs:106、114：sec_bytes = sector_num.to_le_bytes()，buf[0..8].copy_from_slice(&sec_bytes)。
- probe.rs:108-115：check_val 是 (sector_num ^ session_salt ^ pattern_salt) 乘常数再做位混淆，**仍是 sector_num 的确定性函数**。
- probe.rs:118-122：白噪声种子完全由 check_val 派生，没有任何独立秘密。
- probe.rs:221-222：写入的就是这 512 字节。

**上游差异**
libutils.c:224-244 的 fill_buffer_with_block 同样把 offset 明文写进 int64_array[0]（libutils.c:237），libutils.c:234-236 的注释原文：

    /* DO NOT add salt here!
     * Drives know the offset, so applying salt to it leaks the salt.
     */

上游的剩余部分是从 offset ^ salt 起步的 LCG 展开（libutils.c:229、240-243），salt 来自 libprobe.c:899-901 的 srand(time(NULL)) + uint64_rand()。

**结论：本项目不比上游强。** 两者被识别的特征完全一致——前 8 字节就是 LBA。本项目额外做的两件事反而削弱了防御：

1. probe.rs:108 把 sector_num ^ session_salt ^ pattern_salt 乘常数后放进 buf[8..16]，这正是上游 libutils.c:234-236 注释明令禁止的形态（把 salt 和 offset 混进同一个可读结果）。0xD6E8_FEB8_6659_FD93 是奇数，在模 2^64 下可逆，固件拿 (sector_num, check_val) 一步就能解出 session_salt ^ pattern_salt，再配合 buf[16..512] 自洽性即可确认。
2. pattern_salt 只有 0x55 / 0x66（probe.rs:243、247）、0xAA（probe.rs:407、457）、0xBB（probe.rs:515）四个固定值，固件可以成对比较分离出 session_salt。

**攻击者如何利用**
固件对每次写入做一次 8 字节比较（代价可忽略）即可判定"这是探针"。识别到探针后，固件可以让这些扇区真正落盘、让冲刷块静默丢弃，从而使基准探针永远完好、本扇区探针永远正确，探测全程判正品。

**修复方向**
用 keyed PRF（SipHash-2-4 或 ChaCha8，密钥只在内存中）派生整扇区，并且第一个 8 字节不要等于 LBA 本身；pattern_salt 改为由 session_salt 派生而不是固定常量。

---

### P-04（Medium）session_salt 不落盘确实让固件无法离线预计算，但对在线固件只是一层薄屏障；且 salt 的熵源只有系统时钟

**现象**
probe.rs:302 的 session_salt 在进程内生成，全仓检索（src 目录内 session_salt 的全部 15 处引用）只出现在 probe.rs 与 algo.rs 的形参，没有任何序列化到磁盘的路径；src/core/salt.rs 的 SaltRegistry（salt.rs:25、78-105）持久化的是 H2W 会话的 salt，与 probe 的 session_salt 无关。因此探测期间固件确实拿不到会话盐。

**证据**
- probe.rs:95-101：generate_runtime_salt 取 SystemTime::now() 的纳秒数，异或两个**编译期常量** 0x6A09E667F3BCC908 与 0xBB67AE8584CAA73B。整个函数没有读取任何硬件熵源（没有 rand、没有 BCryptGenRandom、没有 RDRAND）。
- probe.rs:302：每次 run 生成一次，随后作为形参传给 407/457/515 三次 test_sector 调用。
- probe.rs:131-148：verify_probe 用同一个 session_salt 重算期望值，与磁盘上不存盐的设计一致。

**上游差异**
libprobe.c:901 的 rwi.salt 来自 uint64_rand()（libprobe.c:310-323，两次 rand() 拼 64 位），libprobe.c:899 用 srand(time(NULL)) 播种。上游同样不落盘，同样不读取硬件熵，且熵源只是**秒级时间戳**。

**关于"不可逆"的判断**
对"离线预计算"这一类对手，不落盘确实是真实增益：固件在固件开发阶段无法算出任何一次运行会用的盐。但对"在线固件"，它只是一层薄屏障——固件在运行时有完整的读写通路，可以看到 buf[0..8] 的 LBA 与 buf[8..16] 的 check_val，按 P-03 的分析一步反解出 session_salt ^ pattern_salt；pattern_salt 只有四个固定值，试四遍即可得到 session_salt。所以准确的表述是"比上游更难一点，但不是密码学意义上的不可逆"。另外 probe.rs:118-122 的位移-乘加链（x ^ (x>>12) 后乘常数）对 64 位整数同样是可逆的，白噪声序列完全由 check_val 决定，没有任何额外秘密。

**攻击者如何利用**
固件在第一次看到某个探针扇区时反解出 session_salt，之后整场探测的探针内容对它就完全可预测，可以针对性地"只对探针路径做缓存、不做真实落盘"。

**修复方向**
改用硬件熵源（Windows 上 BCryptGenRandom 或 RtlGenRandom）生成 session_salt，并用 keyed PRF 而不是"可逆代数 + 位移乘加"派生整扇区。

---

### P-05（Medium）初始排空窗口本身是安全的，但它证明了"固定窗口"思路在回绕介质上不可靠；逐点窗口才是不安全的那个

**现象**
题目关心的是 probe.rs:342-348 的初始排空会不会在极小盘上写穿基准探针，从而把正品盘误判为回绕盘。逐个边界核对的结果是：**初始排空这一段是安全的**，不安全的是逐点窗口。

**证据与边界核对**
- probe.rs:342：initial_scratch = AUX_PROBE_SECTOR + CACHE_FLUSH_BLOCK_SECTORS = 4096 + 128 = 4224。
- probe.rs:343：initial_depth = CACHE_FLUSH_MIN_SECTORS.min(total_sectors - 4224) = 2048（在 total_sectors >= 6272 时）。
- 所以初始排空恒写逻辑区间 [4224, 6272)。

对 2048 与 4096 两个基准扇区：4224 > 4096，窗口起点严格高于两个基准，下溢已被 saturating_sub 与 min 拦住（probe.rs:343）。在**未回绕的正品盘**上，逻辑区间的终点 6272 小于标称容量（MIN_SAFE_SECTORS = 8192 已在 probe.rs:265 拦截更小的盘），所以窗口不会越过盘尾，也不会折回低位，**不会覆盖 2048/4096**。结论：正品盘不会被初始排空误判。

但在回绕介质上，窗口会按真实容量 R 折叠。R = 10240 扇区（5 MiB）时，[4224, 6272) 覆盖物理 4224..6271，仍在 R 内，不折回；R = 4096 扇区时才会折回，但此时标称也不足 MIN_SAFE_SECTORS，被拦截。所以初始排空这一段确实安全。

真正的风险在逐点窗口：probe.rs:200-208 的 scratch_window 用 flush_depth（最大 16384 扇区 = 8 MiB）作为跨度，当真实容量 R 小于 8 MiB 时，窗口跨度就超过一整圈，必然覆盖全部物理扇区，包括 2048/4096。R = 4 MiB 时 t = 67,584 的窗口 [51,200, 67,584) 跨度 16,384 = 2R，全覆盖。这正是 P-01 的成因。

**上游差异**
libprobe.c:568-637 的 find_cache_size 先把缓存**测出来**并记录 cache_pos（libprobe.c:911），之后每次 overwhelm_cache 都重写同一段 [cache_pos, cache_pos + cache_size)（libprobe.c:141-142），这一段由 safe_device 覆盖并最终还原（libprobe.c:757-764）。上游的冲刷窗口是固定且实测的，不随采样点上移；本项目每个采样点都换一个窗口，因此在回绕介质上引入了上游没有的污染面。

**攻击者如何利用**
不需要攻击者——任何真实颗粒小于 8 MiB 而标称很大的扩容盘都会走进 P-01 的构造。反过来说，固件也可以主动利用：让真实颗粒选一个能让逐点窗口折叠后避开 2048/4096 的容量，使 Phase 1 全程沉默。

**修复方向**
初始排空可以直接删掉（它对结论没有任何贡献，只消耗一次 1 MiB 写入）；逐点窗口改为照抄上游的"先测量缓存、再在固定区段原地重写"，并把该区段选在远离基准探针的位置。

---

### P-06（Low）overwhelm_cache 每块只写 64KB 却分配了 64KB 缓冲，块边界与缓冲长度相等，实际并无浪费但代码意图与实现不符

**现象**
probe.rs:179-186 先用 64KB 缓冲填满高熵流，随后只写出 buf.len().min(CACHE_FLUSH_BLOCK_SECTORS * 512) 字节。由于 AlignedBuffer::new(64 * 1024)（probe.rs:293），CACHE_FLUSH_BLOCK_SECTORS * 512 = 128 * 512 = 65536，与 buf.len() 相等，min 恒取 65536，逻辑正确。

**证据**
- probe.rs:293：AlignedBuffer::new(64 * 1024)。
- probe.rs:44：CACHE_FLUSH_BLOCK_SECTORS = 128。
- probe.rs:186：block_bytes = buf.len().min((CACHE_FLUSH_BLOCK_SECTORS * SECTOR_SIZE as u64) as usize)。

**上游差异**
libprobe.c:102-131 的 write_blocks 用 dbuf_get_buf 取一个**动态大小**的缓冲区（libprobe.c:111），一次写入尽可能多的块（libprobe.c:104-113），没有"固定 64KB 再切片"的结构。上游的单次写入量由 flow 的剩余额度与目标区间共同决定。

**攻击者如何利用**
无直接攻击面：当前两个常量相等，min 恒取 64KB。但如果日后有人把 CACHE_FLUSH_BLOCK_SECTORS 调大（例如为了加强冲刷而改成 512 = 256KB），min 会静默截断回 64KB，冲刷量只有预期的四分之一，而代码不会报错、不会有测试覆盖（现有测试 test_probe_abort_during_cache_flush 在 probe.rs:597 预先置位 cancel_flag，走的是取消路径，从未真正写过冲刷块）。

**修复方向**
删除多余的 min，直接用 buf.len() 计算块长；或者把缓冲大小改为由 CACHE_FLUSH_BLOCK_SECTORS 派生，二者取同一个源。

---

### P-07（Low）尾部核验只测一个点，且该点与 Phase 1 最高阶梯之间可能隔着数百 GB 无采样区

**现象**
probe.rs:437-478 的尾部核验只测 total_sectors - 128 这一个扇区。JUMP_LADDER 最高一级是 268,435,456 扇区（约 128 GiB，probe.rs:37），因此对超过 128 GiB 的标称容量，Phase 1 最后通过的采样点是 268,437,504，之后直接跳到盘尾。

**证据**
- probe.rs:16-38：JUMP_LADDER 共 21 项，最大 268,435,456。
- probe.rs:385-387：test_sec >= total_sectors 即 break。
- probe.rs:440：tail_sec = total_sectors.saturating_sub(CACHE_FLUSH_BLOCK_SECTORS)，只测这一个点。
- probe.rs:457-466：尾部核验失败即 is_counterfeit = true，suspected_right = tail_sec。

**上游差异**
libprobe.c:645-662 用 k = ceiling_log2(*pright_pos - good_block) - m 精确算出需要测试的采样个数（libprobe.c:659-662），保证 2^(m+k) + good_block >= *pright_pos，即**整个 [good_block, *pright_pos) 区间都被采样覆盖**，不存在"中间隔着几百 GB 无采样"的空档。

**攻击者如何利用**
真实颗粒 200 GiB、标称 1 TB 的扩容盘：Phase 1 在 128 GiB 通过，随后尾部点失败，suspected_right = 1TB - 64KB，safe_left = 128 GiB，二分区间跨 800 多 GiB。二分中点全部落在 128 GiB 以上（越界区），此时若固件把越界写做成静默丢弃、读回全 0，本扇区探针失配会返回 BitBucket（probe.rs:253-255），二分会正确收缩；但若固件让越界区读回"恰好正确"，二分就会一路右移，收敛到接近盘尾，真实容量被报告成 1 TB。

**修复方向**
阶梯不设固定上限，改为一直翻倍到超过 total_sectors；尾部核验至少采 SAMPLING_MIN 个随机点，并且采到的第一个坏点就直接收缩右界。

---

## 2 对必查项的直接回答

**(a) 上游 ow/check/overwhelm 的行为与常量**
见第 0 节。要点：上游没有 overwrite 函数，等价概念是 libutils.c:292-293 的 bs_overwritten；overwhelm_cache 在 libprobe.c:135-143，冲刷量与位置都是 find_cache_size 实测值；关键常量为 SAMPLING_MIN = 8（libprobe.c:513）、SAMPLING_MAX = 1024（libprobe.c:514）、probabilistic_test 采样 64（libprobe.c:413）、MAX_CACHE_SIZE_BYTE = 1 GiB（libprobe.c:566）、bit_error_tolerance = 7（libutils.c:266）。

**(b) 多倍回绕会不会越过第一个失配点**
以"真实 8 GB、标称 64 GB"为例，8 个回绕点分别在 LBA 16,777,216 / 33,554,432 / 50,331,648 / 67,108,864 / 83,886,080 / 100,663,296 / 117,440,512 / 134,217,728（= R 的 1 到 8 倍）。推演结论：**这一组不会越过**。Phase 1 在 t = 16,779,264 时物理落点恰为 16,779,264 % 16,777,216 = 2048，即采样点自身就写在与主基准相同的物理扇区上，base_ok 立刻为 false，判 WrapAround；suspected_right = 16,779,264，safe_left = 8,390,656，二分 12 步收敛到 16,777,216，误差 0。

但是换成非 2 的幂的小真实容量（33 MiB）就会越过，而且越得很远：safe_left 被推到 1,050,624 扇区（真实边界 67,584），二分在纯越界区间收尾，报告 792 MiB，高估 24 倍。机制见 P-01：每个"通过"点的排空窗口在折叠后恰好没有碰到基准扇区，而它们本身早就越界。所以答案是：**2 的幂容量不会越过，非 2 的幂小容量会系统性越过**。

**(c) 探针可预测性**
可以识别，且识别成本与上游相同：两边的前 8 字节都是明文 LBA（本项目 probe.rs:114，上游 libutils.c:237）。本项目额外把 salt 混进了 buf[8..16]（probe.rs:108），这正是上游 libutils.c:234-236 注释禁止的形态，反而更易反解。**本项目并不比上游强。**

**(d) session_salt 不落盘是否真的对固件不可逆**
不是密码学意义上的不可逆，只是"更难一点"。探测期间磁盘上确实没有盐，离线预计算被挡住；但固件在线看到 buf[0..8] 与 buf[8..16] 后，一步反解出 session_salt ^ pattern_salt，配合四个固定 pattern_salt 试四遍即可恢复 session_salt。白噪声（probe.rs:118-122）的生成链对 64 位整数同样可逆，不构成额外秘密。此外熵源只有系统时钟（probe.rs:96-100），没有硬件随机数。

**(e) probe.rs:250 一律判 WrapAround 的后果，以及 :342-348 与 :200-208 的边界是否安全**
:342-348 的初始排空**是安全的**：窗口恒为 [4224, 6272)，严格高于两个基准扇区，正品盘上不会越界也不会折回，不会把正品盘误判成回绕盘。:200-208 的逐点窗口**不安全**：它的跨度可达 8 MiB，当真实容量小于 8 MiB 时，一次窗口写入就会绕满整圈、覆盖全部物理扇区（含 2048/4096），把 P-01 的 safe_left 推过真实边界。而 :250 的一律 WrapAround 让这种"窗口污染"与"真实回绕"无法区分，结论类型与容量数值同时失真。

---

## 3 与上游能力对照

| 能力 | 上游 libprobe.c / libutils.c | 本项目 probe.rs | 谁更强 |
| --- | --- | --- | --- |
| 回绕定位 | find_wrap 代数定位，采样前就钳死上界（libprobe.c:639-732） | 无对应步骤，靠采样点撞基准 | 上游 |
| 缓存测量 | find_cache_size 实测缓存大小与位置（libprobe.c:568-637） | 固定 8 MiB 上限，按标称容量算（probe.rs:326） | 上游 |
| 坏块搜索 | 区间随机采样 + 单调收缩（libprobe.c:519-553） | 指数阶梯 + 单点二分，左锚点可被污染推过真实边界 | 上游 |
| 采样区间覆盖 | k 由 ceiling_log2 精确算出，全覆盖（libprobe.c:645-662） | 阶梯上限固定 128 GiB，其上只测一个尾部点 | 上游 |
| 被覆盖与坏块的区分 | bs_overwritten 独立成态（libutils.c:292-293） | 无此档，只有三态 ProbeTrap（probe.rs:50-57） | 上游 |
| 读 I/O 错误 | 单独报错并中止（libprobe.c:148-155） | 混入 base_ok/self_ok，变成假盘结论（probe.rs:237-247） | 上游 |
| 数据备份还原 | safe_device 全程备份并恢复（libprobe.c:757-764 注释） | 无 | 上游 |
| 位黑洞识别 | 无专门处理 | 有 self_ok 判定（probe.rs:253-255） | 本项目 |
| 双基准低位校验 | 只读 good_block（libprobe.c:716） | 读 2048 与 4096 两个点（probe.rs:242-247） | 本项目（但被 P-01 与 P-03 削弱） |
| 探针盐 | rand() 64 位（libprobe.c:901，秒级播种） | 纳秒时钟异或常量（probe.rs:95-101） | 平手，都非硬件熵 |
| 探针是否带明文 LBA | 是（libutils.c:237） | 是（probe.rs:114） | 平手 |

---

## 4 建议的验证用例（供后续补强，本次未运行）

1. 用 MemoryBlockDevice::with_fake_wrap 构造标称 64 GiB / 真实 33 MiB，断言 real_size 落在 30~36 MiB。当前实现预计返回约 792 MiB（P-01）。
2. 同为标称 64 GiB / 真实 8 GiB，断言 trap == WrapAround 且 real_size == 8 GiB。当前实现应当通过（已在推演中验证收敛误差 0），可作为回归基线。
3. 构造"基准扇区读错误"的 MemoryBlockDevice 变体，断言不返回 WrapAround（P-02 修复后的契约）。
4. 把 CACHE_FLUSH_BLOCK_SECTORS 改成 512 跑 test_probe_abort_during_cache_flush，验证 P-06 的静默截断是否被测试捕获。
