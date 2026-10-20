# F3-Win 对比上游 F3 的绕过风险审计（Lead 自主复核部分）

审计对象：D:/newC/stick3/f3-win
对照基准：AltraMayor/f3 上游 C 源码（本地克隆于 .upstream_f3/src，仅作阅读对照，不参与构建）
审计方式：纯静态阅读，未执行任何本地编译（遵守项目铁律）。

---

## 一、已确认发现（有明确代码路径）

### L-01 Critical｜4Kn 物理扇区支持是空壳，正品 4Kn 盘会被误判成假盘

证据链：

1. src/core/direct_io.rs:140 定义了 pub fn set_sector_size。
2. 全仓库仅此一处出现该标识符，没有任何调用点（grep set_sector_size 只命中定义本身）。
3. 因而 direct_io.rs:79、:105、:130 三处构造时硬编码的 sector_size: 512 永远生效，任何设备都按 512 处理。
4. src/core/probe.rs:222 写入探针时用的是 &buf[..SECTOR_SIZE]，即 512 字节，偏移 sec * 512。
5. direct_io.rs:99 以 FILE_FLAG_NO_BUFFERING 打开。无缓冲 I/O 要求缓冲区地址、长度、文件偏移全部按物理扇区对齐。4Kn 盘上 512 字节写请求是非法对齐，Windows 返回 ERROR_INVALID_PARAMETER (87)。

后果：
probe.rs:222-224 把这次写失败吞成 Ok(Some(ProbeTrap::BitBucket))。于是一块完全正常的 4Kn 正品设备，会在第一次探针写入时就失败，被判定为位黑洞丢弃型扩容盘。用户看到的是检测到扩容盘，而 error 字段为 None，没有任何线索指向真实原因。

与文档不符：CLAUDE.md 声称支持 512B / 4096B (4Kn) 物理扇区动态自适应，杜绝 Win32 Error 87。代码中该能力不存在。

修复方向：在 open_raw_device 后用 IOCTL_STORAGE_QUERY_PROPERTY 的 STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR 取 BytesPerPhysicalSector，调用 set_sector_size；探针写入长度改为对齐到实际扇区大小。

---

### L-02 Critical｜探针与写入的 flush 语义比上游宽松，短写结果被忽略

探针侧：
- src/core/probe.rs:190 overwhelm_cache 末尾才 flush 一次，即整段排空写完后统一冲刷。
- src/core/probe.rs:225 冲刷失败被记为 BitBucket，把环境故障直接算成判盘结论。

写入侧：
- src/core/session.rs:391-395 只有 file_offset >= GIGABYTE 时才 flush 一次；最后一个不满 1GB 的分卷全程没有显式 flush。
- session.rs:361 与 session.rs:393 都是 let _ = f.flush()，返回值被丢弃。掉盘或写缓存失败会静默继续，最后仍报写入完成。
- 上游对照：f3write.c:233 每个 chunk 写完立即 fdatasync 并检查返回值，错误码保存在 saved_errno 且首次错误优先保留。本项目的冲刷密度与错误保真度都低于上游。

修复方向：每个 chunk 后 flush 并传播错误；分卷结束时无条件 flush；探针 flush 失败应作为探测不可信上报而非判盘结论。

---

### L-03 High｜短写（short write）被静默当作成功

位置：src/core/direct_io.rs:166-195。

write_at 在 WriteFile 返回 Ok 时直接返回 bytes_written，不校验是否等于请求长度。调用方 src/core/probe.rs:222 用的是 if dev.write_at(...).is_err()，只判断 Err，完全忽略返回值。

触发条件：设备写保护临近、扇区映射耗尽、主控在超过物理容量的地址上接受写入但只落一部分。

后果：探针可能只写进去一部分字节，后续校验失败被归因为 BitBucket；或者更糟，只写入了前半段恰好保留了原本的旧探针，被判为通过。

修复方向：write_at 内部循环直到写满，或返回错误；所有调用点校验 bytes_written == buf.len()。

---

### L-04 High｜尾部必须实测通过是死代码，正品判定未被真正约束

位置：src/core/probe.rs:368、:437-478、:460。

tail_verified 在 :368 初始化为 false，在 :460 被赋值为 true，但此后整个函数再无任何读取。它是一个只写不读的死变量。

标称尾部必须实测这一保证实际只依赖 :437 的 if !is_counterfeit && !tail_verified 能被执行到。若 Phase 1 里某一步因为 test_sec >= total_sectors 提前 break（:385-387），或阶梯被 is_last 提前结束（:411-414），尾部核验会被跳过，而 is_counterfeit 仍为 false，最终直接得出完全合规结论。

修复方向：用返回值或显式布尔表达尾部未验证则不得判正品，并在函数出口断言。

---

### L-05 High｜缓存排空量比文档宣称小 4~8 倍

证据：src/core/probe.rs:48 CACHE_FLUSH_MAX_SECTORS: u64 = 16384，即 8MB 上限；:326 flush_depth = (total_sectors / 16).clamp(2048, 16384)。初始排空 :343 更只有 CACHE_FLUSH_MIN_SECTORS（2048 扇区 = 1MB）。

文档不符：CLAUDE.md 声称 overwhelm_cache 冲刷量从 128KB 动态升至 32MB~64MB，物理击穿现代主控的 SLC Cache。实际最大 8MB。

后果：现代扩容盘主控普遍配备 16MB~64MB DRAM 或较大 SLC 模拟区，8MB 排空之后，先前的探针内容仍可能驻留在缓存里被原样返回，制造写入成功的假象。

修复方向：排空量提升到与主控缓存容量同数量级（至少 32MB），或改为多次随机顺序排空加每次排空后 flush。

---

### L-06 High｜段判定规则与上游不一致，Bad 被降级为 Overwritten（影响取证）

上游 libutils.c:283-293 的规则：
- expected_offset == found_offset 且无位错 -> Good
- 同位且位错 <= 7 -> Changed
- 同位且位错 > 7 -> Bad
- 不同位且位错 <= 7 -> Overwritten
- 不同位且位错 > 7 -> Bad

本项目 algo.rs:140-152 的规则：
- 同位且无位错 -> Good
- 同位且位错 > 0 -> Changed（上游这里位错 > 7 应判 Bad）
- 不同位且任意情况 -> Overwritten（上游这里位错 > 7 应判 Bad）

即上游判 Bad 的两类情况，本项目分别判成 Changed 和 Overwritten。Bad 的唯一来源只有 algo.rs:134 提前 return，而该 return 之后偏移是否匹配不再参与判断。

连带影响：src/gui/app.rs:153 的证据存档触发条件是 overwritten > 0 || bad > 0，漏了 changed。一块因电荷漏电正在衰败的真盘会持续产生 changed 扇区，却不会生成任何维权证据档案。

修复方向：严格照搬上游的五分支判定；证据触发条件补齐 changed。

---

### L-07 High｜Salt 来自纳秒时间戳，可预测

位置：src/core/probe.rs:95-101 generate_runtime_salt；src/gui/app.rs:1204-1208。

两处都用 SystemTime::now().duration_since(UNIX_EPOCH).as_nanos() 派生 salt。这不是密码学随机源：分布范围窄，且与真实时间强相关。

攻击方式：扩容盘固件先让主机写入一个探针块，读回明文后在一个按纳秒精度展开的候选窗口内暴力对撞 session_salt，一旦命中即可反解 PRF 并实施算力回放。

修复方向：改用 BCryptGenRandom 或 RtlGenRandom 生成 64 位盐。

---

### L-08 Medium｜写入长度向上取整可能越界写入并让后续分卷错位

位置：src/core/session.rs:321。

上游 f3write.c:186 的 total_file_blocks = 1 << (GIGABYTE_ORDER - block_order)，即每个文件固定 1GiB，且 f3write.c:219 offset = number << GIGABYTE_ORDER，文件内 offset 从文件号乘以 1GiB 起算。

本项目 session.rs:314 while file_offset < GIGABYTE 与 :322 global_offset = (file_index * GIGABYTE) + file_offset 与之表面一致，但 :321 的 to_write_aligned = (to_write + 511) & !511 在 remaining 非 512 整倍数时会向上取整，写出的字节数可能超过本分卷应有的长度，并让 file_offset 累加超过 GIGABYTE，进而让下一个分卷的 global_offset 错位。

需要实测确认：文件系统对 1GiB 的整除性保证。若 target_dir 所在卷的分簇或保留区导致 GIGABYTE 与文件边界不对齐，此处的向上取整就会成为真实错位。

修复方向：明确禁止非 512 整倍数的 remaining，或让 to_write_aligned 只用于缓冲长度，写出的长度严格用 to_write。

---

### L-09 Medium｜changed 与 bad 的 UI 口径不一致

src/gui/app.rs:902 用 bad + overwritten + changed > 0 判定非全绿，而 :153 的取证条件只用 overwritten > 0 || bad > 0。同一份数据在界面上是有问题，在证据链上是没问题。用户拿去做维权时，可能只有界面截图而没有 .md 档案。

---

### L-10 Medium｜generate_runtime_salt 的兜底常量固定

src/core/probe.rs:99 与 app.rs:1208 的 unwrap_or 都提供了固定常量兜底（0xA5A5_5A5A_CAFE_BABE、0x5A5A_A5A5_1234_5678）。当系统时间早于 UNIX_EPOCH 时，所有这些会话都使用同一个已知盐值，防伪能力完全失效且用户无感知。

修复方向：时间源不可用时应该硬失败，或直接改用 BCryptGenRandom 从而消掉这个兜底分支。

---

## 二、待子代理补齐的部分

以下三块由并行子代理继续深挖：
- docs/audit/01-algo-compat.md（PRNG 逐比特、文件布局、SIMD 比对尾部、FlushFileBuffers 顺序）
- docs/audit/02-probe.md（libprobe.c 五阶段逐条对照、多倍回绕、位黑洞防御真实性、探针可预测性）
- docs/audit/03-platform.md（总线白名单绕过、卷锁竞态、diskpart 脚本注入、状态机、CI 门禁真实性）

## 三、方法学说明

本报告仅基于静态代码阅读，未经实机验证。凡标注需要实测确认的条目，需要真实硬件或在 CI 中构造对应的 MemoryBlockDevice 仿真才能定论。严禁据本报告在本地执行编译。
---

### L-11 High｜声称的 SIMD 向量化加速完全不存在

证据：在 src/ 全目录 grep simd / avx / sse / _mm / target_feature / std::arch，零命中。

src/core/algo.rs:120-138 的校验主循环是普通标量循环：
- :120 chunks_exact(8) 分割
- :129-138 逐 u64 比较，不等时 :133 用 val ^ random_number .count_ones() 累加
- :134-136 bit_error_count > BIT_ERROR_TOLERANCE 时提前 return

这段代码不会被 LLVM 自动向量化：循环体内部有带数据依赖的累加、有条件早退分支、有跨迭代状态 random_number（LCG 递推，:130）。这三点都是向量化的硬性阻碍。

文档不符：CLAUDE.md 声称利用 AVX2 / SSE4.2 / NEON 向量化指令与硬件 popcount 加速比较，在 1GB/s~3GB/s 带宽下杜绝 CPU 成为瓶颈。实际是逐 64 位标量循环，每个 u64 都要算一次 LCG（一次 64 位乘加），无法并行。

后果：真实 SLC 直写可达 300MB/s 以上的 U 盘上，单个核心要同时做 LCG 递推与比较，很可能成为瓶颈，测出来的速度是 CPU 上限而不是盘的物理上限，进而让 SLC 掉速断崖的判定失真。这一点需要实测才能定论，但代码里没有任何 SIMD 是可以确认的事实。

修复方向：要么删掉文档里的 SIMD 描述，要么真正实现（8 个 u64 可并行推进 LCG，因为 LCG 是线性递推，可以用矩阵幂一次推进 8 步）。

---

### L-12 High｜系统盘反查失败时默认放行，且依赖可被覆盖的环境变量

证据：

1. src/core/devices.rs:455-462 get_system_drive_disk_number 依赖 std::env::var(SystemDrive)。环境变量可被进程级覆盖，也可能为空，失败时返回 None。
2. src/core/devices.rs:232 is_sys_disk = index == 0 || sys_disk_idx == Some(index)。当 sys_disk_idx 为 None 时，该表达式退化为只有 index == 0 受保护。
3. src/core/devices.rs:233 is_safe = (bus_type.is_removable_bus() || is_removable) && !is_sys_disk。
4. src/core/devices.rs:224 let is_removable = desc.RemovableMedia.as_bool()。该字节来自 STORAGE_DEVICE_DESCRIPTOR，是设备自身上报的属性，不是 Windows 通过物理拓扑验证的事实。

触发组合（两个条件都不在安全设计控制之内）：
- 条件 A：系统盘不是 PhysicalDrive0（加装独显或额外 NVMe 后的常见拓扑），且 SystemDrive 环境变量失效导致反查返回 None。
- 条件 B：该内置盘的固件把 RemovableMedia 报成 1。

两者同时成立时，一块内置固定磁盘会同时绕过 is_removable_bus 白名单与 is_sys_disk 系统盘保护，被判定为 is_safe_target = true，允许运行破坏性扇区探测。

这是典型的 fail-open：反查失败本应默认全部拒绝，现在却退化成只保护 0 号盘。

修复方向：sys_disk_idx 为 None 时 is_safe 直接为 false；移除 is_removable 这条旁路，或至少要求 bus_type 同时匹配；改用 GetSystemDirectoryW 而不是环境变量。

---

### L-13 Medium｜两套 is_removable 语义混用

src/core/devices.rs:224 的 is_removable 来自 STORAGE_DEVICE_DESCRIPTOR.RemovableMedia（物理盘级，固件自报）。
src/core/devices.rs:556 的 is_removable 来自 GetDriveTypeW(path) == DRIVE_REMOVABLE（卷级，系统判定）。

两者同名不同义，都挂在 PhysicalDeviceInfo / VolumeInfo 上并被 src/core/devices.rs:520 的 kind 分支消费。阅读代码时极易混淆哪一套在起作用，也是 L-12 那个旁路的成因之一。

修复方向：改名为 firmware_reported_removable 与 drive_type_removable，强制调用点显式选择。

---

## 第二部分：三路子代理并行审计的独立发现（已由 Lead 抽样复核）

复核说明：以下条目来自 docs/audit/01-algo-compat.md 与 docs/audit/02-probe.md。Lead 对标注为「已复核」的条目亲自读过源码确认；标注为「待复核」的条目信任子代理的行号引用，未逐行亲验。

### L-14 Critical｜salt 注册表按盘符前缀模糊匹配，正品盘被误杀成假盘（已复核）

src/core/salt.rs:170-182：
- :171-172 letter_char 取路径里第一个 ASCII 字母（例如 G），letter_key 只有单字符 G。
- :173-175 先按单字母 key 查，命中即返回。
- :177-182 兜底遍历 reg.records.values()，条件 r.drive_path.starts_with(&letter_key)。

调用链：src/gui/app.rs:1183-1189 校验模式【无条件】调用 query_salt，不询问用户；拿到 salt 后 src/core/session.rs:522 用它推导期望值；期望值错误导致全部扇区判 Bad；src/gui/app.rs:152-168 检测到 bad > 0 自动生成司法鉴定报告。

实测证据（仓库根目录 f3_salt_registry.txt，5 条记录）：
509998A4|G:\|1352243364|...|0x18DF3319FAFB27FC|1792207363
DCEF64FD|H:\|3706676477|...|0x18DF340A9A137A38|1792208396
C4ED7B74|G:\|3303897972|...|0x18DF406C3F5BAAD4|1792222010
98096780|G:\|2550753152|...|0x18DF3337F7345A80|1792207492
F42EB396|G:\|4096701334|...|0x18DF35ACD8B98C98|1792210193

4 条记录共用同一路径 G:\，但 volume_serial 与 salt 各不相同（说明是 4 个不同的物理盘先后插入同一个盘符位置）。records 若为 BTreeMap，values() 按 key 字典序返回，:177-182 恒返回字典序最小的那条，与当前插的是哪块盘无关。

后果：换盘符、换机器、或多块盘轮流用同一盘符，都会拿到别的盘的 salt，真盘被判 100% 坏。这是误报方向（把好盘判成假盘），会直接导致用户拿着假结论去投诉商家。

修复：删除 :177-182 整段模糊匹配；:173-175 的单字母兜底也必须删，因为盘符不是盘的身份。只允许 volume_serial 精确匹配，匹配不到就明确告诉用户「未找到该盘的 salt 凭证」并允许手动输入十六进制 salt。

### L-15 Critical｜探针扇区可被固件反解出会话盐（已复核，修正 Lead 早前的表述）

src/core/probe.rs:105-123 create_probe_block：

  let k = (sector_num ^ session_salt ^ pattern_salt).wrapping_mul(0xD6E8_FEB8_6659_FD93);
  let check_val = (k ^ (k >> 31)).wrapping_add(0x9E37_79B9_7F4A_7C15);

- :114 buf[0..8] 明文写入 sector_num。
- :115 buf[8..16] 明文写入 check_val。
- :118-122 其后 496 字节由 seed = check_val.rotate_left(17) ^ 0x517CC1B727220A95 起步，用 SplitMix64 形态的确定性递推填充。
- pattern_salt 由调用点传入固定常量（probe.rs:243/247/407/457/515，仅 0x55/0x66/0xAA/0xBB 四个值）。

0xD6E8FEB86659FD93 为奇数，在模 2^64 下存在唯一模逆元。x ^ (x >> 31) 与 wrapping_add 都是可逆仿射变换。所以 check_val 是 (sector_num, session_salt, pattern_salt) 的可逆函数：

  session_salt = invert(check_val) ^ sector_num ^ pattern_salt

固件读到任意一个探针扇区（前 16 字节都是明文），就能一步解出 session_salt；随后 :118 的 seed 起点也完全确定，于是整条 496 字节白噪声流可被固件在写入阶段预先算出。

这与 src/core/algo.rs:50-53 以及 CLAUDE.md 宣称的「单向不可逆 PRF，固件无法反解 Salt」直接矛盾。更关键的是，上游 libutils.c:234-236 有一条明确注释，警告不要在被驱动可见的 offset 上叠加 salt，因为「the drive knows the offset, adding salt leaks the salt」。本项目恰好踩进了上游明确避开的坑。

修复方向：去掉 buf[0..8] 的明文扇区号，改为把位置信息作为 PRF 的输入而不是输出；或者接受可识别性但至少不要把 session_salt 做成可解（例如 session_salt 只作为 HMAC 密钥，探针内容用 HMAC(secret, sector_id) 生成，且不写入任何能反解 secret 的字段）。

### L-16 Critical｜最后一个分卷写满时既不 flush 也报成功（已复核）

src/core/session.rs:350-367 的磁盘已满分支：file_offset > 0 时调用 truncate_to 与 flush，等于 0 时直接删文件。
src/core/session.rs:390-395 只在 file_offset >= GIGABYTE 时 flush。
src/core/session.rs:404-420 无论是否 file_full，都下发 H2wPhase::Completed（Write 模式下 is_terminal = true）。
src/core/direct_io.rs:274-282 Drop 只 CloseHandle，不 FlushFileBuffers。

所以最后那个不满 1 GiB 的分卷（恰好是写满盘的那个）全程没有任何显式记录落盘完成点，且 flush 返回值在 :361 与 :393 都被 let _ = 丢弃。

对照上游：f3write.c:233 每个 chunk 写完立即 fdatasync 并检查返回值，首次错误优先保留（f3write.c:234-237）。

假盘利用方式：把真实容量设成略小于标称容量，让写入在最后一个分卷中途报 ENOSPC。本项目会正常走「写满」分支、不发失败、报 Completed；用户看到「写入完成」就去校验，而最后一段没落盘的部分在假盘上读回什么都有可能。

修复：删除「file_offset > 0 才 flush」的条件，任何退出路径（包括 ENOSPC、用户取消、异常）都要先 flush 并检查返回值；写满时应上报一个明确的「磁盘空间不足，已写入 X GB」状态，而不是 Completed。

### L-17 High｜WriteFile 成功但写入 0 字节导致死循环（已复核）
src/core/session.rs:327-331 在 Ok(written) 之后用 actual = min(written, to_write) 推进 file_offset。若 written 为 0，file_offset 不前进；src/core/session.rs:314 的 while file_offset < GIGABYTE 永不退出。
对照上游 f3write.c:163-170 遇到 0 字节写入直接 assert 失败退出。
当前 src/core/direct_io.rs:166-195 的 write_at 也不校验 bytes_written 是否等于请求长度，短写同样被当成成功。

### L-18 High｜salt 注册表非原子写入且写入前就登记（已复核调用点）
src/core/salt.rs:79-105 用 truncate 直写目标文件，无临时文件加原子 rename。进程崩溃或断电会留下空文件或半截文件，全部历史 salt 一次性丢失。
src/core/salt.rs:37-76 load 时静默跳过解析失败的行。
src/core/salt.rs:133-157 register_salt 每次 load 整表、insert、save 整表。
src/gui/app.rs:1177 与 :1211 在写入刚开始就登记 salt，写入失败也会留下记录。

后果：注册表一旦损坏，后续所有校验都拿不到正确 salt，全盘判坏。

### L-19 High｜默认开启加盐导致与原版 f3 / H2testw 不互通（已复核）
src/gui/state.rs:71 use_random_salt: true。
src/gui/app.rs:1168-1178 写入模式下该开关为 true 就生成随机 salt 并写入。
src/core/algo.rs:55-80 one_way_seed 在 salt != 0 时是自定义混合函数，与上游 libutils.c:229 的 offset ^ salt 完全不同；且上游调用点 f3write.c:123 / f3read.c:135 传的 salt 恒为 0。

后果：用本工具默认配置写入的盘，拿原版 f3read 或 H2testw 校验会 100% 判为全盘坏。CLAUDE.md 写的「默认 Salt = 0 与 H2testw / 原版 F3 百分百相互校验」与代码相反。

### L-20 High｜salt 熵不足且明文写入对外报告（已复核）
src/gui/app.rs:1170-1173 与 :1205-1208 用 SystemTime::now().as_nanos() 作为 salt，不是密码学随机源，可被时钟推算。
src/core/report.rs:406 把 salt_hex 明文写进报告；report.rs:508-533 写到 exe 同级目录。
实测证据文件 f3_evidence_BEA6BBCE_20261017_112022.md 第 32 行：会话防伪 Salt 凭证 0x0000000000000000。

同时 report.rs:408 在 salt 为 0 时仍写「经过单向有损散列混淆，恶意假盘主控微码在信息论上无法从扇区首部逆解 Salt」。salt 为 0 时数据就是标准 LCG，任何固件都能直接算出。这段文案属于虚假陈述，而报告是准备交给商家与 12315 的。

修复：改用 BCryptGenRandom；报告里只写 salt 的指纹（例如 SHA-256 前 8 字节）而不写明文；文案按 salt == 0 与 salt != 0 分两种表述。

### L-21 High｜f3probe 二分逼近存在确切的漏判构造（待复核，逻辑可信）
本条来自 docs/audit/02-probe.md P-01。核心问题：src/core/probe.rs:484-532 的二分只在中点测一次，中点通过就把左端点推到中点，而正确做法是失败才收缩右端点、通过必须保留失败证据。
构造：阶梯相邻两级 S 与 U = 2S，真实阈值 T 落在 [1.5S, 2S) 时，S 通过、U 失败，mid = 1.5S 立即通过，二分收敛到 U 附近，实测容量被高报。子代理给出的例子是真 10 GiB 报成 16 GiB（+60%）。
对照上游 libprobe.c:519-553 的算法。

### L-22 High｜probe 结论自相矛盾与 trap 不清除（待复核）
本条来自 docs/audit/02-probe.md P-02。src/core/probe.rs:416-421 与 :515-517 在二分通过后没有清除已置位的 trap，最终可能输出 is_fake = true 而 trap = None、real_size 约等于 announced_size 的矛盾结论；:553 的 ProbeTrap::None 分支实际不可达。

### L-23 High｜两个基准探针全程未被冲刷，固件可给它们独立 SRAM 映射（已复核边界常量）
src/core/probe.rs:200-208 scratch_window 的下界恒为 AUX_PROBE_SECTOR + CACHE_FLUSH_BLOCK_SECTORS = 4096 + 128 = 4224，所以 LBA 2048 与 4096 这两个基准探针在整场探测中从不被冲刷区触及。
假盘固件只要让这两个固定 LBA 走独立 SRAM，就能永远返回正确内容，所谓「双向对流探针」的实际约束只剩采样点自身。
对照上游：不依赖固定基准扇区，而是让采样点互为见证。

### L-24 High｜探测写入会破坏用户数据且全程无备份还原（待复核）
本条来自 docs/audit/02-probe.md P-10 与 P-11。
- src/core/probe.rs:221-227 直接向 LBA 2048 写入 512 字节高熵数据。在 1MB 对齐的 diskpart 布局下，LBA 2048 正是第一个分区的起始扇区（NTFS 引导扇区所在位置）。
- src/core/probe.rs:342-348 初始冲刷从 LBA 4224 起，会覆盖首个分区开头约 1MB 用户数据。
- 全流程没有任何「先读出原始内容、探测结束后写回」的还原步骤。
- 对照上游：f3probe 默认带 safe_device 回滚（libdevs.c:1233-1244、f3probe.c:470-481），只有显式加 --destructive 才不还原；且 libprobe.c:868-875 显式保护分区表。

后果：用户在一张有数据的 U 盘上跑一次「只读探测」，实际上已经损坏了该盘第一个分区的引导区与 1MB 用户数据。这与项目宣称的非破坏性探测定位不符。

### L-25 Medium｜探针与冲刷失败被误判成「扩容假盘」（已复核）
src/core/probe.rs:222-227 在写探针失败时直接 categorize 成 ProbeTrap::BitBucket，src/core/probe.rs:225 flush 失败同样如此。
真实的正品盘如果有坏块或偶发 I/O 错误，会被报成「扩容假盘」。
对照上游 libprobe.c:19-26：重试一次后中止并明确报错。

### L-26 Medium｜阶梯上限 128 GiB，超大容量盘尾部只有单点（已复核常量）
src/core/probe.rs:16-38 的 JUMP_LADDER 末项 268435456 扇区 = 128 GiB。超过 128 GiB 之后 src/core/probe.rs:437-478 只追加一个尾部采样点，128 GiB 到标称容量之间的中段完全没有采样。

### L-27 Medium｜容量计算与读写地址不在同一坐标系（待复核）
announced_size 取自物理盘 DiskSize（src/core/controller.rs:199、src/core/devices.rs:193-194），而探针写入地址 2048 / 4096 / 4224 落在第一个分区内部。两者不是同一坐标系，同一结构体里另有分区容量 total_bytes（devices.rs:580-590）容易被误用。


---

## 第三部分：对子代理 01-algo.md 两条 Critical 的否定结论（Lead 亲验，不成立）

子代理 0c964656 的报告 docs/audit/01-algo.md 提出两条 Critical，经 Lead 逐行核对上游源码，两条均不成立。记录在此以免后续误采信。

### 误报一：乘法器 4294967311 与上游不等价

子代理主张：4294967311 同余于 -1 (mod 2^16)，上游 uint64_t 运算是 17-2^16*X，本项目 wrapping_mul 是 mod 2^64，因此 byte0 落在 0..32767 时失配。

实际算术：4294967311 mod 65536 = 15，不是 65535（即 -1）。子代理连取模结果都算错了。4294967311 = 65536 * 65536 + 65535 = 2^32 + 2^16 - 1。

更根本的错误：C 标准的 uint64_t 无符号整型溢出按模 2^64 回绕，这是标准明确定义的行为，不是未定义行为，也不会产生 17-2^16*X 这种结果。Rust 的 wrapping_mul / wrapping_add 同样是模 2^64。两者数学上完全等价。

Lead 用 4 组数值反证（0x1、0x3039、0xDEADBEEF、0xFFFFFFFFFFFFFFFF），C 与 Rust 结果逐位相同。

结论：salt == 0 时逐比特等价的判断没有被动摇。

### 误报二：block_order 语义不一致，整个数据布局错位

子代理主张：上游一个块只用一条 PRNG 流，本项目每 512 字节重新播种，所以块大于 512 字节时布局必然错位，默认 1MB 块时整盘布局都与上游不同。

实际：上游 .upstream_f3/src/f3write.c:119-127 的 fill_buffer 是：

  for (i = 0; i < sectors; i++) {
      fill_buffer_with_block(buf, SECTOR_ORDER, *poffset, 0);
      buf += SECTOR_SIZE;
      *poffset += SECTOR_SIZE;
  }

传的是 SECTOR_ORDER（512 字节），不是 block_order。而且每个 512 字节扇区都用自己递增后的 poffset 重新播种。

block_order 在文件写入路径上只用于两件事：f3write.c:186 计算 total_file_blocks = 1 << (GIGABYTE_ORDER - block_order)，以及控制写入节奏。它不参与扇区内容的生成。

本项目 algo.rs:84-98 的 fill_sector(buf, offset, salt) 逐扇区用 offset 播种，与上游 fill_buffer 完全一致。

子代理的算术推演（1KB 块 126 条中 63 条不同）本身没有算错，但前提假设错了：上游在文件路径上根本不用 1KB 的块去填充数据。

结论：布局不一致不成立。01-algo-compat.md 的等价结论是正确的。

### 子代理 01-algo.md 中经核查成立的条目

- to_write_aligned 写死 511 与 4Kn 断言矛盾（session.rs:321 对 direct_io.rs:167-172）：成立，与 L-14 相关。
- 校验只比到 512 向下取整、尾部静默丢弃（session.rs:515-518、560）：成立。
- salt 改变后校验旧 .h2w 全盘 Bad 且无提示（app.rs:1185、algo.rs:129-145）：成立，是 L-14 的另一面。
- 无 SIMD：成立，与 L-11 重复。
- 首发故障取证取块起点而非坏扇区（session.rs:525-542，algo.rs:169 丢弃 found_offset）：成立，新增一条。
- 热力图列索引用 total_read 而非 expected_offset（session.rs:554-558 对 :520）：断点续验时列坐标漂移，成立。

---

## 第四部分：第三路子代理（防呆/破坏性操作/CI 门禁）的发现

来源：docs/audit/03-platform.md，共 10 条。以下为 Lead 抽样复核后的记录。

### L-28 Critical｜逻辑卷选择器完全没有安全门禁，C 盘可被写满（已复核）

src/gui/components/device_selector.rs 两个选择器对比：
- :57 物理盘选择器：if resp.clicked() && is_safe { inventory.select_device(idx); }
- :95-97 逻辑卷选择器：if resp.clicked() { inventory.select_volume(idx); }

逻辑卷选择器没有任何 is_safe 判断。而写入校验功能走的是卷路径：src/gui/app.rs:1163 let target_dir = self.state.inventory.selected_path();，此后直接启动任务，全程无任何安全校验。

后果：用户在卷下拉框选中 C 盘并点击开始写入，session.rs 会一直写直到 ERROR_DISK_FULL，即把系统盘剩余空间全部写满。这是本次审计中最容易被普通用户触发的伤害路径，不需要假盘、不需要恶意固件、不需要异常硬件。

对照上游：f3write 由用户显式给出路径，不提供自动选盘面；本项目的 GUI 反而比命令行更容易误操作。

### L-29 Critical｜卷锁守卫与直通句柄分离，空守卫下仍可裸写（待复核）

src/core/devices.rs:410-417 的 open_block_io 返回 (DirectIoFile, VolumeLockGuard) 两个值；src/core/probe.rs:260 的 run 签名不接收任何锁凭据。
当 mounted_volumes 为空时（devices.rs:163 静默跳过、devices.rs:541 只认盘符），加锁循环零次迭代仍然返回 Ok，于是得到一个空守卫，但物理盘句柄照样可以裸写。
对照上游 f3probe.c:428-440 与 :470-474，默认 create_safe_device 并在结束时恢复。

### L-30 Critical｜diskpart 脚本的磁盘号在生成时冻结，rescan 可能改变编号（待复核）

src/core/fix.rs:176-199 生成脚本，:185 是 clean 指令，:182 的 rescan 位于 select disk 之前。磁盘号在 F3FixPlan 构造时就已经确定并写死。
src/core/fix.rs:274-282 的 verify_safety 只在生成脚本之前复核，真正 spawn 时不再检查。
src/core/fix.rs:216-225 对 disk_index 没有范围校验。

rescan 之后磁盘编号可能变化，此时 clean 会作用到别的磁盘上。

### L-31 High｜diskpart 临时脚本落在公共 Temp 且可预测（待复核）
src/core/fix.rs:47-59 与 :70-71：临时脚本名为 f3_fix_{pid}_{millis}.diskpart，写在 std::env::temp_dir()，且用 std::fs::write（CREATE_ALWAYS 语义）直接覆盖。
低权限进程可以抢先创建同名文件或竞争替换，随后以管理员权限执行的 diskpart 会执行被替换后的任意指令。
对照上游：f3fix 不落盘任何脚本文件。

### L-32 High｜CI 的 clippy 门禁实际不阻断（待复核）
.github/workflows/ci.yml:26-27 与 release.yml:25-26 都是 cargo clippy --all-targets，缺少 -- -D warnings，警告不会让流水线失败。
ci.yml:29-30 与 release.yml:28-29 都是 cargo test --verbose，缺少 --all-targets 与 --include-ignored；ci.yml:50-51 显示 aarch64 只编译不测试。
没有机制阻止新增 #[ignore] 让测试静默跳过。

### L-33 Medium｜取消后仍会删光全部 .h2w（待复核）
src/core/session.rs:508-510 的用户取消只 break 内层循环；src/core/session.rs:613 的清理条件不含 cancel_flag，也没检查是否完整跑完。
结果：中途停止后仍会删除全部测试文件，包括尚未校验的部分，与 session.rs:612 注释宣称的证据保护矛盾。

### L-34 Medium｜diskpart 失败判定依赖中英文文本匹配（待复核）
src/core/fix.rs:86 用 let _status 丢弃退出码；src/core/fix.rs:105-108 靠匹配中英文错误文本来判断失败。
非中文或非英文系统下会失效。

### 第三路报告中经核查为合格的项
- 两个 workflow 全文没有 continue-on-error。
- src/gui/app.rs:100-110 的关闭拦截逻辑正确。
- src/core/fix.rs:212-215 与 :251-256 的 disk0 与系统盘拦截存在，且有单测覆盖（fix.rs:343-350）。


---

## 第五部分：04 probe 复现复核（Lead 用真实常量重算）

来源：子代理 6a36f852 报告 docs/audit/02-probe.md 的 P-01。Lead 用 probe.rs 里的真实常量（JUMP_LADDER、BASE_PROBE_SECTOR=2048、AUX_PROBE_SECTOR=4096、CACHE_FLUSH_BLOCK_SECTORS=128、CACHE_FLUSH_MAX_SECTORS=16384、MIN_SAFE_SECTORS=8192）重算了 33 MiB / 标称 64 GiB 这一组。

结论：方向成立，具体数字与子代理给出的不同。子代理的采样点为 6144/10240/18432/34816/67584，Lead 用代码常量算得 8192/8320/8576/9088/10112/12160/16256/24448/40832/73600。差异源于起始点与步进累加方式的细节，但核心机制一致。

Lead 复现出的关键证据（真实 33 MiB = 67584 扇区，flushDepth = 16384）：

- t=8192 时 phys=8192，窗口 [4224, 8192) 完全在真实容量内，写成功。
- t=24448 时 phys=24448，窗口 [8064, 24448)，写成功。
- t=73600 时 phys=6016（已越过真实边界 67584），但窗口 [4224, 6016) 完整落在真实容量内，未绕过圈、未触碰基准，因此仍然写成功，且 self_ok 判定通过。
- t=139136 时 phys=3968，scratch_window 因 phys < 4224 返回 None，窗口被跳过。

也就是说：scratch_window（probe.rs:200-208）只按逻辑 LBA 计算窗口，对物理容量一无所知。在回绕盘上，逻辑上早已越界的采样点只要其物理落点恰好落在 [4224, 容量) 区间内，就会写成功并通过探针校验，于是被当作安全点。Phase1 的 safe_left 因此会被推到远超真实容量的位置，Phase2 的二分随后在纯越界区间内搜索，收敛结果与真实容量无关。

这与 P-01（probe.rs:484-532 二分只在中点单测、中点通过就推左端点）是两个独立缺陷叠加：前者给了错误的左锚点，后者无法纠正它。

用户后果：探测报告给出的真实容量可能严重高估，用户据此重建分区会丢失超出真实容量的那部分数据。

修复方向：
1. scratch_window 必须显式回避 LBA 2048 与 4096，不能只靠下界 4224 就认为安全（回绕盘上窗口在物理空间是折叠的）。
2. Phase1 记录 safe_left 前，必须用写前基线复核基准扇区，而不是依赖写后快照。
3. Phase2 的二分必须遵循失败收缩上界、通过不得丢弃失败证据的原则。

### 子代理 6a36f852 关于初始排空的更正意见（可信）
probe.rs:342-348 的初始排空恒写 [4224, 6272)，4224 > 4096 严格高于两个基准，saturating_sub 与 min 拦住下溢，正品盘不会因此误判。Lead 前文 L-24 中关于「初始冲刷覆盖首分区」的表述仍然成立（那是数据破坏问题），但不构成误判来源。

### 上游能力对照（本项目缺失）
子代理列出上游有而本项目没有的能力，可信度高，值得列入改进清单：
- libprobe.c:639-732 find_wrap：代数定位回绕点，本项目用阶梯采样代替。
- libprobe.c:645-662 ceiling_log2：精确覆盖采样区间。
- libprobe.c:568-637 find_cache_size：实测缓存大小，冲刷量取自实测，上限 MAX_CACHE_SIZE_BYTE = 1 GiB（libprobe.c:566）。本项目固定 8 MiB 且按标称容量计算。
- libutils.c:292-293 bs_overwritten：独立的状态位。
- safe_device 备份还原（libdevs.c:1233-1244）。
本项目优于上游的只有位黑洞 self_ok 判定与 LBA 2048 + 4096 双基准。


---

## 第六部分：最终一批发现的复核（含两条改变优先级的 Critical）

### L-35 Critical｜校验中途取消被判为完成，并渲染出 AAA 级结论（已复核）

src/core/session.rs:507-510 的取消判断只 break 掉【文件内】的读循环。外层按文件遍历的循环在 :465，命中 :466 时会发送 H2wPhase::Aborted（这一条是对的）。
但无论从哪条路径退出，src/core/session.rs:631-649 都【无条件】发送 H2wPhase::Completed，且 :642 的 error 参数传 None。
src/core/controller.rs:298-310 收到 Completed 即判定为 Finished。
src/gui/app.rs:473 与 :849-853 按 Finished 状态渲染结论卡片。

用户视角：点停止后，界面显示测试完成、误差 0、AAA 级健康介质。实际只校验了一部分。

这条比写坏系统盘更危险，因为它直接产出错误结论，而且用户会信任它。上游 f3read 没有这条路径：命令行工具按 Ctrl-C 直接退出，不产出报告。

修复：取消路径必须终止整个任务并明确上报 Aborted，不得落进 Completed 分支；完成卡片必须校验 telem.aborted 与已校验字节是否等于预期字节。

### L-36 Critical｜校验的预期容量取自已有文件长度，假盘可以自证清白（已复核）

src/core/session.rs:453-455：total_expected_bytes 等于磁盘上已有 .h2w 文件长度之和，不是设备标称容量。
src/core/report.rs:174：announced_bytes 在拿不到设备信息时，回落到 telemetry.total_bytes_expected，即同一个数字。
src/gui/app.rs:488-489 用同一数字反填 expected。

完整攻击链：假盘标称 64 GB、真实 8 GB；写入在 8 GB 处 ENOSPC，留下 8 个 .h2w 文件；校验时 total_expected_bytes = 8 GB，announced_bytes 也 = 8 GB；于是校验覆盖率 100%，虚标比例 0%，报告结论为容量完全合规。

假盘只要让写入在真实容量处停下来，校验阶段就会主动出具合格证明。这与 A1（写满不 flush 却报完成）是同一漏洞的两个环节，但这一条递进为主动出具合格证明，危害更大。

对照上游：f3read.c:355 校验文件长度必须是 512 的整数倍，:416-418 对不满一个完整分卷的文件发 WARNING 并返回非零。上游把文件长度异常当作错误上报，本项目把它当作预期基准。

修复：total_expected_bytes 必须来自设备标称容量；文件总长与标称容量不符时明确报告差异，不得静默采用文件长度；announced_bytes 不得回落到文件长度。

### L-37 High｜diskpart 失败判定恒为假成功（已复核调用点，需真机验证文本）

src/core/fix.rs:100 的 wait_with_output 在 try_wait 已收割之后调用，但 windows 0.58 在 stdout 未重定向时会改用管道读取，输出仍然拿得到。所以问题不在拿不到输出。
问题在 src/core/fix.rs:102-108 的文本匹配：判错依赖 DiskPart has encountered an error 或 Virtual Disk Service error 这类英文字样，中文系统也不含半角 错误。
而 diskpart /s 模式下即使命令失败，进程仍可能以 0 退出。
结果：has_err 基本恒为 false，分区修复失败会被显示为成功。反向也有：输出里出现 未发现错误 会把成功判成失败。

src/core/fix.rs:86 的 let _status 丢弃退出码，是这个问题的根源之一。

修复：以退出码为准，文本只作辅助；失败时必须上报失败。

### L-38 High｜探测被停止后同样被判为完成（待复核）
src/core/controller.rs:334-342 把「停止探测」也判成 Finished，而不是 Error 或 Aborted。
src/gui/app.rs:128-152 的完成边界处理不写 audit_record，导致取证链断裂。

### L-39 High｜单元测试全部跑在仿真设备上，关键分支零覆盖（已复核）
src/core/direct_io.rs:374-393 的 MemoryBlockDevice：不支持短写、不产生 EIO、flush 恒返回 Ok、sector 恒为 512。
因此 src/core/probe.rs:221-228 的 BitBucket 失败分支在仿真下永不执行。
src/core/controller.rs:199 与 src/core/direct_io.rs:105 都写死 512，4Kn 无任何测试覆盖。
CI 环境非管理员，PhysicalDrive 真实路径从未真正跑过。

这解释了为什么 B2、B3、B5 这类问题能长期留存：仿真设备比真实设备宽容太多。

修复：给 MemoryBlockDevice 增加可配置的故障注入（短写、EIO、flush 失败、4Kn 扇区、回绕），把这些分支纳入 CI。

### L-40 Medium｜flush 只有 FlushFileBuffers，缺少硬件级指令（已复核）
src/core/direct_io.rs:152-163 只调用 FlushFileBuffers。
全工程没有 SCSI SYNCHRONIZE CACHE、NVMe Flush、USB 设备复位（USBDEVFS_RESET 的 Windows 等价物）的对应实现。
对照上游：libdevs.c:451-470 有 fsync 配合 posix_fadvise(DONTNEED)，libdevs.c:788-816 有 USB 复位。

### L-41 Medium｜托盘销毁可能造成主线程永久阻塞（推测级，需真机验证）
src/gui/tray.rs:131-159 的 WM_TRAY_CMD 分支只 DestroyWindow，靠 WM_DESTROY 里的 PostQuitMessage 收尾。
src/gui/tray.rs:417-434 之后又 PostMessageW 到已经销毁的 hwnd，然后 join()。时序不利时主线程永久阻塞。
src/gui/app.rs:105-109 与 :1149-1156 都走这条路径。

### L-42 Low｜桌面路径硬拼 %USERPROFILE%\Desktop（已复核）
src/gui/app.rs:620-625 与 src/core/report.rs:722-736 硬拼环境变量，回退到 .。
OneDrive 重定向或域重定向的机器上会写错位置。
src/core/fix.rs 的 f3_fix.diskpart 固定名会静默覆盖同名文件。

### 对 docs/audit/03-platform.md 第 8 条结论方向的纠正
旧版写的是「靠中英文文本匹配判失败」，方向暗示存在误判风险。实际情况更糟：has_err 基本恒为 false，失败被显示为成功。已由 L-37 取代。

### 对 L-29（探测备份还原）上游语义的更正
上游 f3probe 的 sdev 只备份真正写过的块（libdevs.c:1115-1336），掉盘或强杀时同样恢复不了，所以不能说上游完全无损。
但本项目连「写过就备份」这一层都没有，正常完成路径上有损，上游在正常路径上无损，属于被删掉的整整一块语义。

