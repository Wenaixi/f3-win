# algo.rs 与 session.rs 数据正确性审计报告

审计范围：`src/core/algo.rs`、`src/core/session.rs`，以及它们直接依赖的 `src/core/direct_io.rs`、`src/core/aligned_buf.rs`。
上游对照基线：`.upstream_f3/src/libutils.c`、`.upstream_f3/src/f3read.c`、`.upstream_f3/src/f3write.c`、`.upstream_f3/src/libfile.c`、`.upstream_f3/src/libprobe.c`。
本报告只收录有确定代码证据的结论，未运行任何编译器。

---

### Critical：byte0 取值在 0..32767 范围内时，写出的扇区在 H2testw 兼容 salt=0 模式下逐条失配，校验必然报 Bad

现象
本项目把 PRNG 的种子改成 `one_way_seed(offset, salt)`。当 `salt == 0` 时该函数直接返回 `offset`（`algo.rs:56-58`），注释声称"与原版 F3 / H2testw 保持 100% 逐比特向后兼容"。但乘法器 `4294967311` 在 mod 2^n 下恒等于 -1。因此当 `offset < 32768`（即小文件的第一个扇区、以及任何 byte0 落在 0..32767 的位置）时，两者等价关系被破坏：

- 上游：`z_1 = X * 4294967311 + 17 = 17 - 2^16·X`（`libutils.c:219-222`，`libutils.c:241-242`）
- 本项目：`z_1 = (X * 4294967311) mod 2^64 + 17`，因为 `wrapping_mul` 在 mod 2^64 下求解（`algo.rs:45`）

乘数之差为 `4294967311 - (2^64 - 1) = 2^64 + 4294967312`，即少了 `2^32 - 16` 倍的 X。两者只要 `X · (2^32 - 16) ≠ 0 (mod 2^64)` 就分叉。对于 1GB 大文件（X 是 GB 倍数）二者恰好重合，因此普通大容量测试不会暴露问题；但只要块偏移的 byte0 落在 0..32767（小容量介质、`--start-at` 断点续测、文件内偏移就在 0 附近），写出的字节流就与原版 F3 / H2testw 不兼容。

证据
- `src/core/algo.rs:45`：`random_number.wrapping_mul(LCG_MULTIPLIER).wrapping_add(LCG_INCREMENT)`
- `src/core/algo.rs:56-58`：`if salt == 0 { return offset; }`
- `src/core/algo.rs:93-97`：首条 seed 后循环写入 63 条
- `src/core/session.rs:324`：`fill_sectors(&mut buf[..to_write_aligned], global_offset, self.config.salt)`
- `src/core/session.rs:322`：`let global_offset = (file_index * GIGABYTE) + file_offset;`

上游差异
- `.upstream_f3/src/libutils.c:219-222`：`return random_number * 4294967311ULL + 17;`（无符号 64 位模运算，语义是 `17 - 2^16·X`）
- `.upstream_f3/src/libutils.c:229`：种子为 `offset ^ salt`，salt=0 时就是裸 offset
- `.upstream_f3/src/libutils.c:240-243`：块内连续 63 条以上

攻击者如何利用
扩容盘固件只需按同一公式在读取时回放前 63 条，即可让"小文件/低位偏移"区段完全通过校验。更关键的是，本项目自身对这批位置也无法用原版 F3、H2testw 交叉复验——兼容性承诺失效正是二手平台"用别的工具查也是好的"话术的立足点。

一句话修复方向
把 `next_random_number` 的乘法语义显式对齐 C 的 `uint64_t` 模运算（本项目 `wrapping_mul` 已经是 mod 2^64，问题在于 `one_way_seed` 在 salt=0 分支中引入了未归一化的 offset），并在 salt=0 分支上补一条与 C 参考实现逐比特比对的单元测试。

---

### Critical：整个数据布局按 512 字节扇区独立重算 PRNG，与上游"整个块共享一个 PRNG 流"的根本语义不一致

现象
上游 `fill_buffer_with_block` / `validate_buffer_with_block` 收到的是一整块（block_order >= 9，实际使用文件系统块，通常 4096），块内所有 `1 << (block_order - 3)` 条 int64 **共用一条 PRNG 流**，种子只在块首取一次。本项目拆成 `fill_sector`，每个 512 字节扇区各自用本扇区 offset 重新播种并重跑 63 条。这不是等价变换，而是一份完全不同的字节布局。

只要块大小大于 512 字节（默认 1MB 缓冲，见 `src/gui/state.rs:73`），块偏移非首个扇区的数据就整体错位。由于 LCG 是 mod 2^64 双射，块内各条 PRNG 值两两不同，"扇区重建值"与"块内位置值"必然分叉——本报告已用纯算术验证：1KB 块内 126 条中有 63 条不同，4KB 长条下 504 条中有 441 条不同。

反过来说，这份实现自身是自洽的：写和读都走 `fill_sectors` / `validate_sectors`，自测自验能通过。问题只在于它无法与上游逐比特对齐，任何"写一端、读另一端"的跨工具验证都会成片失败。这一点必须作为明确的产品结论写进文档，或者改回块级填充。

证据
- `src/core/algo.rs:84-98`：`fill_sector` 每个扇区都执行 `one_way_seed(offset, salt)` 并重跑 63 条
- `src/core/algo.rs:101-111`：`fill_sectors` 按 512 字节切分后逐扇区调用
- `src/core/algo.rs:115-153`：`validate_sector` 同样按扇区独立重建
- `src/core/session.rs:324`、`src/core/session.rs:522`：唯一的两个调用点，块大小由 `chunk_size` 决定
- `src/gui/state.rs:73`：`chunk_size_mb: 1`

上游差异
- `.upstream_f3/src/libutils.c:227`：`const unsigned int num_int64 = 1U << (block_order - 3);`（整块条数）
- `.upstream_f3/src/libutils.c:237-243`：种子只取一次，块内从下标 1 连续跑到 `num_int64 - 1`
- `.upstream_f3/src/libutils.c:263-280`：校验端同样按整块条数 `num_int64` 连续重建
- `.upstream_f3/src/libprobe.c:118-122`：顺序填充时 `stamp_blk` 每块前移 `block_size`，offset 每块加 `block_size`，块内仍是一个整体

攻击者如何利用
此条本身不是提权漏洞，但它决定了兼容性结论：固件无需伪造任何东西，只要在原版 F3 写入、本项目读取（或反之）就能让测试得出错误结论。攻击者（或卖家）可以据此宣称"权威工具 F3 也验不过你的盘是工具的问题"，把真扩容盘的证据链打散。更直接的风险是：用户拿本项目写的数据交给原版 f3read 复验得到满屏 Bad，会误判为正品盘损坏。

一句话修复方向
把 `fill_sector`/`validate_sector` 提升为 `fill_block_with_block(buf, block_order, offset, salt)`，按 `1 << (block_order - 3)` 条整块推进 PRNG；扇区级粒度只用于统计与热力图，不用于生成数据。

---

### Medium：`to_write_aligned` 的对齐运算不跟随物理扇区大小，4Kn 介质下写路径必然断言失败

现象
写入路径用 `(to_write + 511) & !511` 把待写长度向上取整到 512 字节，但 `DirectIoFile::write_at` 的断言用的是 `self.sector_size`。项目为 4Kn 提供了 `set_sector_size`（`direct_io.rs:140-144`）并在 CLAUDE.md 中声称已支持"512B / 4096B (4Kn) 物理扇区动态自适应，杜绝 Win32 Error 87"，但 session.rs 里写死了 511。一旦 `sector_size` 被设为 4096，且 `to_write` 不是 4096 的倍数，`write_at` 的 `assert_eq!` 会直接 panic，而不是返回可报告的 Win32 错误。

证据
- `src/core/session.rs:321`：`let to_write_aligned = (to_write + 511) & !511;`
- `src/core/direct_io.rs:167-172`：`assert_eq!(buf.len() % self.sector_size, 0, ...)`
- `src/core/direct_io.rs:140-144`：`set_sector_size` 仅接受 512 或 4096

上游差异
- `.upstream_f3/src/libfile.c:42-51`：`get_block_order` 从 `statvfs` 的 `f_frsize` 动态取块大小，块大小全程贯穿填充与写入，从无硬编码
- `.upstream_f3/src/f3write.c:163`：`assert((turn_size & (SECTOR_SIZE - 1)) == 0);`——上游也只断言 512，但它从不把二者混用作两套粒度

攻击者如何利用
不是攻击面，是可用性缺陷。恶意介质或 USB-NVMe 桥接芯片上报 4096 逻辑扇区时，程序在写入中途 panic，线程崩溃后 UI 只看到"任务异常结束"而拿不到可读的失败原因，用户会误以为是自己的盘坏了或程序不稳定。当前 `set_sector_size` 在会话路径上没有任何调用点（全仓 grep 只在定义处出现），所以这是"声明了未接通"的状态。

一句话修复方向
`to_write_aligned` 改为按 `writer.sector_size()` 取整，并在 `set_sector_size` 真正接入 CreateFile 之后的 IOCTL 探测结果，再接上 `FILE_FLAG_NO_BUFFERING` 的 4Kn 校验。

---

### Medium：校验路径只统计"读到的整扇区"，文件末尾不满 512 字节的尾巴既不校验也不计入，且 silently 丢弃

现象
`execute_verify` 读回 `bytes_read` 后计算 `aligned = bytes_read - (bytes_read % SECTOR_SIZE)`，只把 `&buf[..aligned]` 交给 `validate_sectors`。一旦末尾有 1..511 字节非整扇区数据，这部分**永远不会被验证**，而且 `total_read` 累加的是未对齐的 `bytes_read`，导致统计口径里这些字节"被读了但没被校验"。

上游在这里是 assert 直接暴露问题的（`f3read.c` 中 `assert((bytes_read & (SECTOR_SIZE - 1)) == 0)`），本项目则是静默截断。考虑到写入路径有 `truncate_to(file_offset)`（`session.rs:360`），而 `file_offset` 是在磁盘满时由 `write_at` 返回值推出来的，理论上始终扇区对齐；但一旦文件被第三方工具、断电或文件系统截断成非扇区长度，本项目的校验就会在无告警的情况下放过尾巴。

证据
- `src/core/session.rs:515`：`let aligned = bytes_read - (bytes_read % SECTOR_SIZE);`
- `src/core/session.rs:516-518`：`if aligned == 0 { break; }`
- `src/core/session.rs:522`：`validate_sectors(&buf[..aligned], expected_offset, self.config.salt, &mut stats);`
- `src/core/session.rs:560`：`total_read += bytes_read as u64;`（统计用了未截断的值）

上游差异
- `.upstream_f3/src/f3read.c` 的 `check_chunk`：`assert((bytes_read & (SECTOR_SIZE - 1)) == 0);` 再 `check_buffer(buf, bytes_read >> SECTOR_ORDER, ...)`——上游对非对齐直接断言失败，不会静默吞掉
- `.upstream_f3/src/f3read.c` 的 `iterate_files`：`assert((tot_stats.ok + tot_stats.bad + tot_stats.changed + tot_stats.overwritten) << SECTOR_ORDER == tot_size);`——上游有一条总账恒等式，本项目没有对应断言

攻击者如何利用
扩容盘固件把最后一个 .h2w 文件的实际长度截短到非扇区边界（例如满盘时让 NTFS 留下 200 字节尾巴），本项目的校验会把该尾巴跳过，同时 `total_expected_bytes` 仍按 `metadata().len()` 计入，进度与"数据 OK"比例被抬高。用户看到"校验完成、无坏扇区"，实际最后一段数据从未被读过。

一句话修复方向
在 `aligned != bytes_read` 时显式报 Bad 并输出一个错误状态（或至少计数进 `changed`），并在会话末尾加一条与上游相同口径的总账断言。

---

### Medium：salt 改变后，旧的 .h2w 文件会把每一个扇区都判为 Bad，程序没有任何提示或保护

现象
`start_h2w_task` 在 `H2wMode::Write` 且勾选随机 salt 时，会用 `SystemTime::now()` 生成新 salt 并写入注册表。如果用户此前用别的 salt 写过 .h2w，或者干脆同时存在两个盐批次，随后直接点"开始校验"，`query_salt` 只会返回注册表里**最后一次登记**的那一个 salt。用错误的 salt 校验：`found_offset` 仍然等于 `expected_offset`（首 8 字节是明文 offset，与 salt 无关，见 `algo.rs:87-90`），但块内 PRNG 值全部对不上，`bit_error_count` 在前几条内就超过阈值 7，`validate_sector` 立刻返回 `Bad`。

结果是全盘 `bad`，而 `changed`/`overwritten` 都是 0。用户看到的是"全盘数据损坏"——这正是本项目最想避免的误报形态，因为它与真正的扩容盘故障表现（局部 bad/overwritten）不同却被展示成同一类结论，维权场景下会直接毁掉证据。程序既不记录写入时的 salt 批次，也不在文件里放无歧义的盐指纹，无法在开始校验前发现"盐不匹配"。

证据
- `src/gui/app.rs:1168-1178`：写入分支每次生成新 salt 并 `register_salt`
- `src/gui/app.rs:1185-1189`：校验分支无条件用 `query_salt` 的返回值覆盖用户输入
- `src/core/salt.rs:155`：`reg.records.insert(key, record);`——同一卷只保留最后一条记录，旧的被覆盖
- `src/core/salt.rs:188-212`：`unregister_salt` 后旧盐彻底不可恢复
- `src/core/algo.rs:129-137`：错误 salt 下 `bit_error_count` 迅速越过 `BIT_ERROR_TOLERANCE = 7`，返回 `Bad`
- `src/core/algo.rs:140-145`：只有 `bit_error_count == 0` 才可能判 Good

上游差异
- `.upstream_f3/src/f3write.c:123`：`fill_buffer_with_block(buf, SECTOR_ORDER, *poffset, 0);`——上游写路径写死 salt=0
- `.upstream_f3/src/f3read.c` 的 `check_sector`：`validate_block_update_stats(sector, SECTOR_ORDER, expected_offset, &found_offset, 0, &stats->secs);`——上游读路径同样写死 salt=0

上游根本没有"盐会变"这个维度，所以盐语义是本项目新增的可变状态，必须自带一致性保护。

攻击者如何利用
真实场景不必是"攻击"：用户在 A 次会话写入（盐 S1），程序随后被关闭、注册表被清理/搬移 exe、或用户手动改 salt，再次校验时全盘报 Bad。扩容盘卖家可以主动引导用户反复重跑测试并改动设置，制造"工具结果不稳定"的口碑问题；而真正被扩容覆盖的数据区本来会表现为 `overwritten`，现在被同屏的"全盘 Bad"噪声淹没。

一句话修复方向
在每个 .h2w 文件头之外加一个非破坏性的侧车记录（或在文件名/注册表里按批次登记 salt），校验开始前先做一次 salt 批次确认；盐不匹配时直接中止并提示"盐不匹配，请用写入时的盐校验"，而不是把结果喂进统计。

---

### Low：CLAUDE.md 声称的 AVX2 / SSE4.2 / 硬件 popcount 加速在 algo.rs 中完全不存在，校验位错误统计走的是标量 `count_ones`

现象
`algo.rs` 与 `session.rs` 全文没有任何 `std::arch`、`is_x86_feature_detected!`、`#[target_feature]`、`_mm`/`_mm256` 内建函数，也没有对应特征门控。填充走 `chunks_exact_mut(8)` 逐条标量写，校验走 `chunks_exact(8)` 逐条标量比对 + `(val ^ random_number).count_ones()`。可移植、正确、编译器自动向量化不能覆盖（因为 PRNG 是严格串行依赖），性能天花板是"每条一次 64 位乘加"，与文档宣称的"杜绝 CPU 成为瓶颈"不符。

有趣的是，上游恰恰**没有**这块优化：上游 `__builtin_popcountll` 与 `int64_array[i] != random_number` 才是真正的"原版行为"。所以这是本项目相对上游的缺失，不是兼容性问题——但它和文档的描述直接冲突，属于可信度风险。

证据
- `src/core/algo.rs:85`：`let mut chunks = buf.chunks_exact_mut(8);`
- `src/core/algo.rs:129-138`：校验主循环，标量逐条
- `src/core/algo.rs:133`：`bit_error_count += (val ^ random_number).count_ones();`
- `src/core/algo.rs:84`：注释 "安全且对齐无关，单指令吞吐"——实际是 63 次串行乘加
- 全仓 grep `AVX2|SSE|simd|std::arch|target_feature` 在 `src/` 下无任何命中

上游差异
- `.upstream_f3/src/libutils.c:272-280`：`__builtin_popcountll(int64_array[i] ^ random_number)`，逐条标量
- `.upstream_f3/src/libutils.c:240-243`：逐条标量 PRNG

攻击者如何利用
不构成攻击面，但构成绩效承诺失真。用户在 1GB/s 以上的介质上会看到 CPU 单核打满而带宽跑不满，进而怀疑"工具不准"，给假盘卖家的辩解留下空间。

一句话修复方向
删除文档中的 AVX2/SSE4.2 表述，或真正对"多条累积异或"部分做 SIMD 归约（PRNG 本身无法并行，可并行的只有最后的比对与 popcount 归约）。

---

### Low：`validate_sector` 的 Bad 判定依赖提前返回，与上游"先算完再分类"的顺序不同，`found_offset` 的返回时机也不一致

现象
本项目在 `bit_error_count > 7` 时立即 `return (BlockState::Bad, found_offset)`，不再继续扫描剩余条。这将导致：只有当错误集中在扇区前部时才会被判 Bad；错误集中在后部时会扫完整个扇区才判。结果状态本身正确（两者最终都是 `Bad`），但**提前返回**导致无法对同一扇区复用"最大错误位置"信息，也无法与上游的 `break` 后统一分类流程逐行对应。

更具体的语义差异是上游的分类表：上游先比较 `expected_offset == found_offset`，在不等分支下 `bit_error_count <= tolerance` 才给 `overwritten`，否则给 `bad`。本项目先看 `bit_error_count == 0`，再看 `expected == found`，分支树形状不同但结论一致（`algo.rs:140-152` 与 `libutils.c:284-295` 在各种输入组合下等价）。

需要指出的是任务清单里提到的"漏掉 found_offset 与 expected_offset 不同时的 bad 分支"**并不存在**：本项目的 `Bad` 优先于 offset 比较返回，覆盖了上游两个 `bad` 出口。此处只记录实现顺序与可维护性差异，不构成正确性缺陷。

证据
- `src/core/algo.rs:134-136`：`if bit_error_count > BIT_ERROR_TOLERANCE { return (BlockState::Bad, found_offset); }`
- `src/core/algo.rs:140-152`：`bit_error_count == 0` 分支树
- `src/core/algo.rs:5`：`pub const BIT_ERROR_TOLERANCE: u32 = 7;`

上游差异
- `.upstream_f3/src/libutils.c:277-278`：`if (bit_error_count > bit_error_tolerance) break;`（break 而非 return）
- `.upstream_f3/src/libutils.c:284-295`：`if (expected_offset == found_offset) { ... }` 与 `if (bit_error_count <= bit_error_tolerance) return bs_overwritten; return bs_bad;`

攻击者如何利用
提前返回本身不影响判定结果，但会让"首发失配现场取证"（`session.rs:525-542`）拿到的 `first_error_offset` 是整块起点而非首个坏扇区，取证精度下降。假盘维权时"第一个坏扇区在哪"是有价值的事实，模糊后削弱了证据。

一句话修复方向
按上游的 break 语义重写，并把 break 时的 `i` 一并返回，供取证使用；分类逻辑照抄上游两次分支比较即可。

---

### Low：`execute_verify` 的"首个故障现场取证"取的是块起点而不是坏扇区本身，实际内容与期望值对不上

现象
`validate_sectors` 返回后，代码用 `expected_offset`（块起点）作为 `first_error_offset`，并从 `buf[..16]` 抓取"实际读回的前 16 字节"，再用 `fill_sectors` 为**块起点**合成"预期首 16 字节"。然而块起点扇区往往正是好的（`stats.bad` 的增加来自块内后面的某个扇区），于是：

- 记录的 `first_error_offset` 指向一个健康扇区；
- `first_error_actual` 是健康扇区的内容；
- `first_error_expected` 也是健康扇区的内容（同一种子合成）。

两者相同，报告里的"hexdump 对照"显示不出任何差异，用户拿到的是无效证据。这份数据流向 `src/core/report.rs` 的维权报告卡片。

证据
- `src/core/session.rs:525-527`：`if first_error_offset.is_none() && (stats.bad > prev_stats.bad || ...)` 时记录 `expected_offset`
- `src/core/session.rs:530-534`：`actual_16.copy_from_slice(&buf[..16]);`
- `src/core/session.rs:537-541`：为 `expected_offset` 合成 `expected_16`
- `src/core/session.rs:169`：`validate_sectors` 内已经丢掉了逐扇区的结果（`let (state, _) = validate_sector(...)`）

上游差异
- `.upstream_f3/src/libutils.c:282`：`*pfound_offset = found_offset;`——上游把**失配扇区自己记录的 offset** 回传，这才是取证需要的信息
- `.upstream_f3/src/libutils.c:298-323`：`validate_block_update_stats` 把 `pfound_offset` 一路带出

攻击者如何利用
扩容盘覆盖写通常表现为 `overwritten`：扇区里记录的是另一个合法 offset。上游正是靠 `found_offset` 反推出"这个扇区实际存的是哪一段数据"，从而定位覆盖范围。本项目把 `validate_sector` 的返回值 `found_offset` 在 `validate_sectors` 里直接丢弃（`algo.rs:169` 的 `let (state, _) =`），现场取证能力被削弱到零，假盘卖家可以质疑"你连坏扇区在哪都指不出来"。

一句话修复方向
让 `validate_sectors` 回传首个非 Good 扇区的索引与 `(state, found_offset)`，session 用它生成 `first_error_offset` 和 hexdump 对照。

---

### Low：上游有 `fill_buffer` 的块级 offset 推进与 `assert(bytes_written == turn_size)` 的整块写保证，本项目 `write_at` 部分写入后仍按"实际量"推进，可能与缓冲区内容脱节

现象
`execute_write` 里 `write_at` 成功后取 `actual = min(written, to_write)` 并推进 `file_offset`。Direct I/O 的 `WriteFile` 要么全写要么报错，理论上 `written == to_write_aligned`，但代码允许 `written < to_write_aligned` 的路径存在：此时缓冲区里 `[actual..to_write_aligned)` 的数据从未落盘却被跳过，下一轮会以新的 `global_offset` 重新填充，数据本身没错（因为按 offset 重算），但 `total_written` 与进度会短暂偏小。这属于健壮性问题，不是数据损坏。上游对此是硬 assert。

证据
- `src/core/session.rs:328-331`：`Ok(written) => { let actual = std::cmp::min(written, to_write); ... file_offset += actual as u64; }`
- `src/core/session.rs:321`：`to_write_aligned` 可能大于 `to_write`（末块向上取整）

上游差异
- `.upstream_f3/src/f3write.c:170`：`assert(bytes_written == turn_size);`——上游绝不接受部分写入

攻击者如何利用
恶意 USB 桥接芯片可以返回一个小于请求长度的"成功"写入量（部分写入），本项目的进度条会照常推进并最终报告"写入完成"，而磁盘上对应区间是旧数据。此后立刻做校验会得到 `overwritten`/`changed`，但报告里"写入成功"的结论已经下过，用户容易把后续失败归因为校验环节而非写入环节。

一句话修复方向
在 `written != to_write_aligned` 时按上游的做法直接判失败并上报，不要用 `min` 掩盖。

---

### Low：`validate_sectors` 忽略 `SECTOR_SIZE` 之外的区块边界，热力图取样用 `total_read` 而非 `expected_offset`，断点续验时列坐标会漂移

现象
热力图列索引由 `total_read / total_expected_bytes * 999` 计算（`session.rs:554-557`）。`total_read` 是本次会话累计已读字节，而校验的实际位置是 `(*num * GIGABYTE) + file_offset`（`session.rs:520`）。当用户用 `start_file_num` 从中途开始续验时，`total_read` 从 0 开始，但文件的实际物理位置可能在第 5GB——热力图会把后半盘的数据画在左侧。这不是数据正确性问题，但会让用户对"哪个区段坏了"产生错误认知。

证据
- `src/core/session.rs:427-433`：按 `start_file_num` 过滤文件
- `src/core/session.rs:453-455`：`total_expected_bytes` 只累计被选中的文件
- `src/core/session.rs:554-558`：`let cell_idx = ... total_read / total_expected_bytes ...`
- `src/core/session.rs:520`：`expected_offset = (*num * GIGABYTE) + file_offset;`

上游差异
- `.upstream_f3/src/f3read.c` 的 `check_sector`：只用 `expected_offset` 驱动校验，没有任何列归一化（上游无热力图）
- `.upstream_f3/src/libfile.c:101-110`：`include_this_file` 用 `start_at <= *number <= end_at` 过滤，语义与 `start_file_num` 一致

攻击者如何利用
扩容盘在前 4GB 正常、后 4GB 回绕时，若用户从第 3 个文件开始续验，热力图会把"回绕区"标在前 1/3 处，与用户实际看到的物理位置错位。卖家可以说"你的测试图自相矛盾"。

一句话修复方向
用 `expected_offset / (volume_capacity)` 而不是 `total_read / total_expected_bytes` 计算列索引，或在续验时把 `total_read` 初始化为 `start_file_num * GIGABYTE`。

---

## 附：逐项答复必查清单

(a) block_order 语义：**不一致**，且是本报告第一、二条 Critical。上游是块内单流，本项目是扇区级多流，任何块大小大于 512 字节的情况下字节流都不同。详见第二条。

(b) `validate_sector` 是否漏掉 `found_offset != expected_offset` 时的 bad 分支：**没有漏**。本项目用"Bad 优先返回"覆盖了上游两个 `bad` 出口，结论等价。但实现顺序与上游不同，可维护性较差，见 Low 条目。

(c) SIMD：**不存在**。`src/` 全仓无 `std::arch` / `is_x86_feature_detected!` / `#[target_feature]` / `_mm*` 任何痕迹。CLAUDE.md 中的 AVX2/SSE4.2 主张完全没有代码支撑，硬件 popcount 也未显式启用（`count_ones()` 依赖编译器自动生成 popcnt，且没有 `-C target-feature=+popcnt` 门控）。

(d) 校验比对范围：只覆盖 `bytes_read` 向下取整到 512 的整扇区部分，文件末尾 1..511 字节的尾巴被静默跳过，且 `total_read` 用的是未截断值造成口径不一致。见 Medium 条目。

(e) 改 salt 后校验旧 .h2w：全部扇区判为 `Bad`（`found_offset` 仍匹配，但 PRNG 全错），程序不会提示盐不匹配，注册表同一卷只保留最后一条记录，旧盐不可恢复。见 Medium 条目。
