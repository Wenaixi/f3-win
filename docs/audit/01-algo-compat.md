# F3-Win 写入/校验核心算法与上游 f3 逐比特兼容性审计

审计范围：本项目执行写入与校验的核心算法链路，以及它与上游 f3 原仓库在数据格式与判定语义上的一致性。
只做静态阅读与推理，未编译任何代码，未修改任何源码。
证据基准：.upstream_f3/src/ 下的 f3write.c、f3read.c、libflow.c、libfile.c、libutils.c、libutils.h。

---

## 0. 结论速览

1. salt == 0 时，本项目的扇区数据生成与上游 f3 / H2testw 逐比特等价。已逐字节推演头若干个 64 位字并对照，无差异。
2. 扇区分类判定与上游互相一致（good / changed / bad / overwritten 四态、7 位容差、提前中断），逐分支对照无差异。
3. 真正的兼容性断裂点全部集中在边界与外部输入上：4Kn 扇区未生效、salt 注册表跨卷串味、salt 注册表丢记录、尾部分卷被写满、salt 随手加导致上游工具无法校验、salt 泄露且熵不足。
4. 影响最大的两条（H-2 与 C-1）不是算法错误，而是 salt 存取策略与写入结束语义的错误。

严重度统计：Critical 1 条，High 5 条，Medium 7 条，Low 3 条。

---

## 1. 已确认等价的部分

列出这一节是为了说明审计确实做到了逐行，不是为了给项目背书。

### 1.1 常数与数据类型

- 本项目：src/core/algo.rs:2-9，SECTOR_ORDER = 9，SECTOR_SIZE = 512，NUM_INT64_PER_SECTOR = 64，LCG_MULTIPLIER = 4294967311，LCG_INCREMENT = 17，BIT_ERROR_TOLERANCE = 7。
- 上游：.upstream_f3/src/libutils.h:8 SECTOR_ORDER 9，libutils.h:14 SECTOR_SIZE (1ULL << SECTOR_ORDER)，libutils.c:221 `random_number * 4294967311ULL + 17`，libutils.c:266 `bit_error_tolerance = 7`。
- 乘加表达式：本项目 algo.rs:45 使用 wrapping_mul / wrapping_add；上游为 uint64_t 无符号乘法，C 标准规定无符号溢出按模 2^64 回绕。二者数学上完全一致，不存在符号扩展或 32 位截断。

### 1.2 首值推演（人工手算对照）

上游 libutils.c:229 `random_number = offset ^ salt`；libutils.c:237 `int64_array[0] = offset`；libutils.c:240-243 从 i=1 起每个字先推进再写入。
本项目 algo.rs:88-97 先写 offset，再令 `random_number = one_way_seed(offset, salt)`，然后循环内先推进再写。

salt == 0 时 one_way_seed 直接返回 offset（algo.rs:56-58），因此：

- 第 0 个字：offset
- 第 1 个字：(offset ^ 0) * 4294967311 + 17 = offset * 4294967311 + 17
- 第 2 个字：(offset * 4294967311 + 17) * 4294967311 + 17

与上游 i=1、i=2 两条语句展开后的结果完全相同。轮数都是 63 次推进（第 0 字除外），两边一致。

结论：salt == 0 分支逐比特等价，已确认。algo.rs:49 的注释是准确的。

### 1.3 校验的分类判定

上游 libutils.c:284-295：
- expected == found 且 bit_error 为 0 -> good
- expected == found 且 bit_error <= 7 -> changed
- expected == found 且 bit_error > 7 -> bad
- expected != found 且 bit_error <= 7 -> overwritten
- expected != found 且 bit_error > 7 -> bad

本项目 algo.rs:134-152：超过容差立即返回 Bad；否则 bit_error == 0 且 expected 相等时判 Good；expected 相等判 Changed；expected 不等一律判 Overwritten。

逐分支比对：唯一差异是上游对 offset 不等且超容差的情形返回 bad，本项目在超容差时已提前返回 Bad（algo.rs:134-136），结果相同。结论：四态判定与上游一致，已确认。

### 1.4 部分被改的块能否检出

上游比对本块内全部 63 个随机字；本项目 algo.rs:129 的 chunks 迭代同样覆盖第 1 到第 63 个字，比较长度都是 504 字节，没有只比前 N 字节的问题。

一个块若只有最后几十个字节被改，落在第 62、63 个字里，仍然会被比对到并判为 Changed 或 Bad。

关于 SIMD：本项目实际代码里没有 AVX2/SSE 路径，algo.rs 全部是标量 u64 比对，因此不存在 SIMD 尾部残余字节被跳过的问题。CLAUDE.md 宣称的 SIMD 向量化在写入/校验核心算法中并未落地，属于文档与实现的偏差，本身不影响兼容性。若日后补上 SIMD，必须保证尾部 504 字节的残余部分不被跳过。

### 1.5 文件全局 offset 的推导

上游 f3write.c:219 `offset = number << GIGABYTE_ORDER`；f3read.c:264 `expected_offset = number << GIGABYTE_ORDER`；number 是文件编号减一（libfile.c:79 num + 1 用于文件名，libfile.c:98 num - 1 用于解析）。
本项目 session.rs:322 `global_offset = (file_index * GIGABYTE) + file_offset`；session.rs:520 `expected_offset = (*num * GIGABYTE) + file_offset`；session.rs:148 解析时同样减一。

两边都是文件序号减一乘以 1 GiB 加上块内偏移，块内偏移按 512 递增（上游 f3write.c:125 `*poffset += SECTOR_SIZE`）。已确认一致。

关键点：上游的块内偏移与 PRNG 种子没有关系，与扇区大小也没有关系，种子只由全局 offset 决定。这直接决定了 H-1 那条发现的性质。

---

## 2. 已确认的缺陷

### H-1 4Kn（4096 物理扇区）自适应没有接入，若按 4096 对齐就会与上游数据错位

- 严重度：High
- 状态：已确认（代码路径可指认）

代码位置：
- src/core/direct_io.rs:24-26 trait 默认返回 512；direct_io.rs:79、105、130 三处创建句柄时都把 sector_size 硬编码为 512。
- direct_io.rs:140-144 定义了 set_sector_size，但全仓库 grep 只有定义处命中，没有任何调用点。
- direct_io.rs:167-172 与 direct_io.rs:199-204 的 assert 用的是 self.sector_size，当前恒为 512，这条一致性断言对 4Kn 介质没有约束力。
- 对照上游：libfile.c:42-51 get_block_order 用 statvfs 的 f_frsize 决定块序，f3write.c:164 与 f3read.c:193 按该块序切块，但 f3write.c:123 与 f3read.c:134 校验时始终用 SECTOR_ORDER 即 512。上游对 4Kn 介质没有任何按物理扇区改写数据布局的处理，块内偏移恒为 512 的整数倍。

触发条件：在物理扇区为 4096 的介质上，若有人把 set_sector_size(4096) 接上，或者 WriteFile 因 512 非 4096 整倍数返回 ERROR_INVALID_PARAMETER（87）。

攻击者利用：
1. 环境异常（最现实）：当前 sector_size 恒为 512，若目标卷要求 4096 对齐，写读会直接失败，用户看到 Direct I/O 写入失败 (Win32 错误码 87)。这不是假盘绕过，而是功能不可用。
2. 假盘固件：一旦未来有人补上 set_sector_size 并把 offset 改成 4096 步进，而不改种子派生，写出的数据布局就与上游 f3read 完全错位，上游工具全盘报 Bad。
3. 真正要记住的是语义约束：上游的种子函数只依赖全局字节 offset，任何为对齐而对 offset 做的取整或步进改动，都会破坏与上游的互换性。

修复方向：不要按物理扇区改 offset 步进；4096 对齐只用在缓冲区长度与起始地址上（offset 仍按 512 递增），并补一个 4Kn 下与上游交叉校验的用例。

---

### H-2 salt 注册表按盘符模糊匹配，换盘符或跨机器就会把真盘误杀成假盘

- 严重度：High
- 状态：已确认（代码路径与仓库内实际数据均可指认）

代码位置：
- src/core/salt.rs:160-185 query_salt：先按卷序列号精确匹配；匹配不到时，salt.rs:171-181 退化为取盘符首字母后遍历所有记录做 `r.drive_path.starts_with(&letter_key)` 前缀匹配，命中即返回该记录里的 salt。
- src/gui/app.rs:1183-1189 校验模式无条件走 query_salt，把返回值当作本次会话的 salt。
- src/core/session.rs:522 用这个 salt 推导本卷所有扇区的期望值。

触发条件：
1. 同一块物理盘换了盘符（G 变 H、或卷被重新格式化换了序列号）。
2. 盘插到另一台电脑上，注册表不在那台机器上（注册表存在 exe 同级目录，salt.rs:28-34）。
3. 注册表存在但不是本次写入时写的那一条。

仓库里的实际证据（已读取 D:/newC/stick3/f3-win/f3_salt_registry.txt）：

```
509998A4|G:\|1352243364|1792207363394709500|0x18DF3319FAFB27FC|1792207363
DCEF64FD|H:\|3706676477|1792208396856031800|0x18DF340A9A137A38|1792208396
C4ED7B74|G:\|3303897972|1792222010380364500|0x18DF406C3F5BAAD4|1792222010
98096780|G:\|2550753152|1792207492180368000|0x18DF3337F7345A80|1792207492
F42EB396|G:\|4096701334|1792210193203432600|0x18DF35ACD8B98C98|1792210193
```

五条记录里四条盘符都是 G:\，序列号各不相同。这不是异常数据，而是同一台机器上反复换 U 盘、都用 G 盘符的常态。

后果推演：一个 G 盘，序列号 F42EB396，本机注册表里没有它的精确记录，query_salt 走 salt.rs:177-182 的前缀匹配，返回的可能是 509998A4 那一条的 salt。用错误的 salt 校验，本卷每个扇区的期望随机序列都错掉，全部判为 Bad 或 Overwritten，报告会写成 100% 损毁、确认为严重容量虚标的伪劣扩容假盘。这是把真盘定性成假盘，并且自动生成一份看起来极其正式的司法鉴定 Markdown。

反向风险同样存在：假盘只要让固件把卷序列号伪造成注册表里已有的值（卷序列号是文件系统元数据，由主控完全可控），就能让本工具用上一次的旧 salt 去校验这块盘，从而掩盖真正的 salt 差异。

攻击者：
1. 用户误操作：换盘符、换机器、重新格式化都可能触发。
2. 假盘固件：伪造卷序列号命中注册表中的旧记录。
3. 环境异常：注册表文件被误删或未备份。

修复方向：前缀匹配（salt.rs:177-182）必须删除，查不到就明确报错并要求用户输入 salt，绝不猜；salt 应绑定到写入三元组而不是只绑序列号。

---

### H-3 salt 注册表整文件读写、无原子替换，且第二条写入路径会整表覆盖

- 严重度：High
- 状态：已确认

代码位置：
- src/core/salt.rs:37-76 load：读整个文件解析，解析失败的行静默跳过。
- src/core/salt.rs:79-105 save：OpenOptions 用 write + create + truncate 直接截断原文件后写入，不是写临时文件再 rename 的原子替换。
- src/core/salt.rs:133-157 register_salt：每次写 salt 都是 load 整表、insert 一条、save 整表。
- src/core/session.rs:324 与 session.rs:522 都用同一个 salt 覆盖整卷。

触发条件：
1. 写入过程中（可能几小时）进程崩溃、断电、被任务管理器结束，save 执行到一半，注册表被截断成空文件或残缺文件。
2. 两台机器或两个实例同时写同一个注册表。
3. 用户手工编辑过这个 txt。

后果：注册表丢失后，原本能校验通过的 .h2w 文件再也无法校验，用户只能看到全盘 Bad。这与 H-2 是同一后果面的两个诱因。

自动化影响：app.rs:1177 与 app.rs:1211 在开始写入的那一刻就已经登记了新 salt，若写入中途失败或用户取消，注册表里留下一条指向没有对应数据的 salt 记录，下一次校验依然会拿到它。

修复方向：save 改成写临时文件加 MoveFileEx 替换；并且注册表里存在 salt 不等于卷上数据是用该 salt 写的，需与最后一次成功的写入完成事件绑定。

---

### C-1 最后一个分卷写满（ENOSPC）时不会 flush，写入以完成收尾，且覆盖范围不可见

- 严重度：Critical（对 数据确实落到了物理介质上 这一核心保证而言）
- 状态：已确认（代码路径可指认）

代码位置：
- src/core/session.rs:350-367 写满分支：is_disk_full 时调用 truncate_to(file_offset) 与 flush()。注意 flush 只在 file_offset > 0 的 if 分支里调用（session.rs:359-361）。
- src/core/session.rs:362-366 另一分支：刚创建就满时丢弃句柄删除文件，没有 flush。
- src/core/session.rs:390-395 只有 file_offset >= GIGABYTE 才 flush。
- src/core/session.rs:397-399 file_full 为真即 break 出外层循环。
- src/core/session.rs:404-420 最后发送 H2wPhase::Completed 且 is_terminal = true，没有任何因磁盘满而提前结束的标记。
- 对照上游 f3write.c:233 fdatasync(fd) 是在每一轮 chunk 写完都调用，f3write.c:267-297 把 ENOSPC 与正常完成区分开来。
- src/core/direct_io.rs:274-282 Drop 里只有 CloseHandle，没有 FlushFileBuffers，关文件时的最终冲刷完全依赖 Windows 自身行为。

触发条件：目标盘剩余空间恰好不是 GiB 的整数倍，这是绝大多数 U 盘的现实情况。

后果：
1. 写入被磁盘满截断，却当成写入完成上报（session.rs:404-418）。用户以为测了整盘，实际末尾一小段没有被写入也没有被校验。
2. 未写入的区域对假盘来说是安全的：假盘只要把真实容量设成比标称略少一点，让写满发生在末尾，就不会有任何坏扇区被检出。
3. 上游行为是明确的：ENOSPC 单独上报，且每个 chunk 后 fdatasync。

攻击者：用户误操作 + 环境异常 + 假盘固件（把真实容量设成略小于标称）。

修复方向：写满截断后必须显式 FlushFileBuffers，并在遥测里用独立字段区分写满结束与写完结束，UI 与报告必须显示实际覆盖的字节范围。

---

### H-4 salt 由 SystemTime::now().as_nanos() 生成，熵极低且明文写入存证报告

- 严重度：High
- 状态：已确认

代码位置：
- src/gui/app.rs:1170-1173 与 app.rs:1205-1208：SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64)。
- src/core/report.rs:406 把 salt 明文写进 Markdown 报告：会话防伪 Salt 凭证：{}。
- src/core/report.rs:508-533 auto_archive_evidence 把报告写到 exe 同级目录。
- 仓库里 f3_evidence_BEA6BBCE_20261017_112022.md 第 32 行就是明文：会话防伪 Salt 凭证：0x0000000000000000。
- 对照上游 libutils.c:234-236 的原始注释：DO NOT add salt here! Drives know the offset, so applying salt to it leaks the salt. 上游的 salt 参数在所有调用点都是 0（f3write.c:123 传 0，f3read.c:135 传 0）。

触发条件与利用：
1. 熵值问题：纳秒时间戳不是密码学随机数，as_nanos 的值落在 1.79e18 附近即 2^61 量级，高 3 位基本固定，实际熵远低于 64 位。恶意固件若知道写入的大致时间（测试时长、购买时间、报告时间戳都可见），搜索空间会收窄到几十位以内。
2. 更致命的是明文泄露：报告里 salt 既有十六进制（report.rs:406）又有十进制（report.rs:590），而报告会作为维权证据发给商家、平台、12315、法院。固件厂商拿到报告，salt 就等于公开，one_way_seed 再非线性也没有意义，对方不需要反解，直接抄答案即可让 compute-on-read 通过。
3. 报告目录就是 exe 同级目录，仓库根目录下已堆积 4 份 f3_evidence_*.md，.gitignore 拦截了入库，但文件确实存在于磁盘上。

这条直接推翻了 report.rs:408 那句 Salt 凭证仅留存于宿主机系统盘、绝对未向被测介质落盘的保证：它没有落到被测介质上，但它落到了会被公开的文件上。

修复方向：salt 用 BCryptGenRandom（系统 CSPRNG）生成；报告里只保留 salt 的 SHA-256 前 8 字节指纹用于溯源，绝不写明文。

---

### H-5 salt != 0 时与上游 f3 / H2testw 完全不互通，且默认开启

- 严重度：High
- 状态：已确认

代码位置：
- src/gui/state.rs:71 use_random_salt: true，注释为默认开启单向安全加盐。
- src/core/algo.rs:55-80 salt != 0 时走 one_way_seed 的非线性置换；上游的唯一种子公式是 libutils.c:229 random_number = offset ^ salt，且所有实际调用点 salt 都传 0。
- src/core/session.rs:324 与 session.rs:522 直接用配置里的 salt，没有任何本次输出是否与上游兼容的标记传递到 UI 或报告。

触发条件：默认配置下点击开始写入。

后果：
1. 写出的 .h2w 文件用上游 f3read 读会 100% 报 bad。用户拿我用了 F3 测试的说法去维权时，对方用官方 f3 一跑全红，反而证明用户的数据是损坏的，处境更糟。
2. 用 H2testw 的读取部分同样不通过。
3. 上游 f3write 写的文件，本工具用默认配置去校验也会全盘报 Bad。注意 app.rs:1198 的 else 分支返回 0 是对的，但只要用户此前用本工具写过一次同盘符的盘，就会拿到错误 salt（H-2）。

这里没有算法错误，问题在于默认值与宣称的矛盾：CLAUDE.md 说默认 Salt = 0 与 H2testw / 原版 F3 百分百相互校验，而代码默认是随机 salt。

修复方向：默认 salt = 0；把加盐防作弊做成显式开关，并在报告里标注本报告数据与原版 f3 / H2testw 不兼容。

---

### H-6 WriteFile 返回 Ok 但写入 0 字节时，写入循环空转，任务永久卡住

- 严重度：High
- 状态：已确认（代码路径可指认）

代码位置：
- src/core/session.rs:327-331：Ok(written) => { let actual = min(written, to_write); total_written += actual; file_offset += actual; }
- src/core/session.rs:314 while file_offset < GIGABYTE 循环，file_offset 不再增长即无限循环。
- src/core/direct_io.rs:187-195 只要 WriteFile 返回 Ok，就返回 bytes_written，不检查是否为 0。

触发条件：某些桥接或异常固件在内部错误时返回成功但 bytes_written == 0；或写入被上层过滤驱动截获后返回标称成功。

后果：写入循环空转，重复生成相同数据，UI 冻结、ETA 与进度永久停住。取消标志在 session.rs:315 有检查所以能退出，但在此之前用户看到的是任务假死。这不是假盘绕过，而是健壮性缺陷，但攻击者可用它做拒绝服务式掩盖。

上游对应行为：f3write.c:167-170 `if (rc != 0) goto out;` 然后 `assert(bytes_written == turn_size);`，写不满就是断言失败，绝不会空转。

修复方向：Ok 但 bytes_written == 0 时立即判为失败并上报。

---

### M-1 分卷过滤与上游 ls_my_files 对 0.h2w 的处理不一致

- 严重度：Medium
- 状态：已确认

代码位置：
- 上游 f3write.c:64-68 start_at = ll - 1，f3write.c:74-77 end_at = ll - 1，libfile.c:109 闭区间判断，语义为 [start-1, end-1]。
- 本项目写入 session.rs:253 与 session.rs:275-279 闭区间，与上游一致。
- 本项目校验 session.rs:427-432 闭区间，与上游一致。
- 但 session.rs:287 文件名是 format!("{}.h2w", file_index + 1)，session.rs:148 解析 num - 1，session.rs:147 `if num > 0` 使得解析为 0 的 0.h2w 被直接丢弃。上游 libfile.c:60-74 is_my_file 接受 0.h2w（只要求首字符是数字）。

触发条件：目录里存在 0.h2w（上游工具不会产生，但早期版本或用户手工复制可能产生）。

后果：两边的文件集合不同，校验总数与百分比统计对不上；0.h2w 在本工具里被静默忽略，用户以为校验了 N 个文件，实际是 N+1 个里有 1 个没被检查。

修复方向：与上游保持一致的处理，显式拒绝 0.h2w 并在 UI 报告里列出被忽略的文件名，而不是静默丢弃。

---

### M-2 校验时对 最后一个不满 1 GiB 的分卷 与 文件被截断 没有任何判断

- 严重度：Medium
- 状态：已确认

代码位置：
- src/core/session.rs:512-518：Ok(0) => break 把 EOF 当作正常结束；let aligned = bytes_read - (bytes_read % SECTOR_SIZE); if aligned == 0 { break }。
- src/core/session.rs:453-455：total_expected_bytes 用 std::fs::metadata(...).unwrap_or(GIGABYTE)，metadata 失败时直接假设这个文件是 1 GiB。
- 对照上游 f3read.c:355 `assert((st.st_size & (block_size - 1)) == 0)`，f3read.c:352-353 要求必须是常规文件，f3read.c:416-418 对范围内文件缺失给出 WARNING，f3read.c:426-427 返回码把 bad+changed+overwritten 与 not_read_all 都算作失败。

触发条件：文件在写入后被截断；或文件末尾有不完整扇区。

后果：本工具安静地少校验尾部，不报告文件长度异常；上游会明确 WARNING 并以非零返回码表示失败。上游能报警、本工具不报，属于互通性问题中确有实证的一条。

攻击者：假盘固件把最后一次写入的数据丢弃，让文件长度短一截，本工具照样报 Good，只是少算几个扇区。

修复方向：校验开始前对每个文件做长度检查，末尾不是 512 整倍数时直接判为失败并报告文件名与长度。

---

### M-3 校验时按 file_index + 1 显示当前文件号，start_file_num > 0 时分段校验显示错位

- 严重度：Medium
- 状态：已确认

代码位置：src/core/session.rs:469 与 session.rs:566 都用 (file_idx + 1)，file_idx 是筛选后 Vec 的下标；上游 f3read.c:418 报的是实际 number。

触发条件：用户用断点续验从第 100 个文件开始。

后果：界面显示正在校验第 1/50 个文件，实际是第 100 个。不影响判定，但会让用户误以为校验了整个盘。

修复方向：用 files[file_idx].0 + 1 而不是下标。

---

### M-4 上游 f3read 打开文件后 fdatasync 并丢弃页缓存，本工具只读打开且不冲刷

- 严重度：Medium（对硬件级穿透的宣称而言）
- 状态：已确认

代码位置：
- 上游 f3read.c:249-257 fdatasync(fd)，失败时直接 exit；f3read.c:258 与 f3read.c:261 用 posix_fadvise(POSIX_FADV_DONTNEED) 与 POSIX_FADV_SEQUENTIAL 主动丢弃页缓存。
- 本项目 src/core/direct_io.rs:115-137 open_read 只带 FILE_FLAG_NO_BUFFERING，没有任何丢弃缓存的动作。
- src/core/direct_io.rs:274-282 Drop 只有 CloseHandle。

触发条件：读写缓存、桥接芯片缓存、SLC 缓存。

后果：FILE_FLAG_NO_BUFFERING 只绕过 Windows 卷缓存，不能绕过主控的 DRAM/SLC 缓存。假盘固件可在缓存层做手脚：写入时数据进到 DRAM 缓存就算成功，读取时命中缓存就直接返回缓存内容，在缓存容量范围内无论真实颗粒是否存下数据校验都能通过。这是与算法无关的绕过窗口。

修复方向：校验前对目标卷下发一次卷级 flush，并在读路径中间插入远距离随机重读（做成可选，代价是速度）。

---

### M-5 写入端无总容量上限保护，剩余空间取 GetDiskFreeSpaceExW 且失败时按 0 处理

- 严重度：Medium
- 状态：已确认

代码位置：src/core/session.rs:228-250，max_bytes 为 None 时读 GetDiskFreeSpaceExW，if ok.is_ok() { free_avail } else { 0 }；session.rs:252 把 0 当作总目标；session.rs:281-285 只在 max_bytes 为 Some 时才检查上限。

触发条件：GetDiskFreeSpaceExW 失败（路径含非法字符、卷在写入过程中被卸载）。

后果：写入会一直继续直到系统返回 ENOSPC，而不是提前刹车；ETA 与进度全错。

修复方向：GetDiskFreeSpaceExW 失败时直接中止任务并报错，不要退化为 0。

---

### M-6 salt == 0 与 salt != 0 两种模式下报告的防算力回放证明文案完全相同

- 严重度：Medium
- 状态：已确认

代码位置：src/core/report.rs:406-408，无论 salt 是 0 还是非 0 都输出拒绝算力回放、单向有损散列。实际证据文件 f3_evidence_BEA6BBCE_20261017_112022.md 第 32-34 行就是 salt = 0x0000000000000000 的样本，文案仍宣称单向有损散列、信息论上无法逆解。

后果：salt = 0 时数据就是纯 LCG，固件用 libutils.c:229 的 offset ^ salt 一步就能推出来。这份报告作为司法证据提交时技术描述与实测数据不符，属于伪证性质的错误。

修复方向：报告里的算法描述必须根据 salt 是否为 0 条件分支输出。

---

### M-7 尾部分卷写入量按 512 向上取整，可能超出分卷边界（潜伏）

- 严重度：Medium
- 状态：已确认（计算路径），当前配置下不触发

代码位置：src/core/session.rs:319-321：

```rust
let remaining = GIGABYTE - file_offset;
let to_write = std::cmp::min(buf.len() as u64, remaining) as usize;
let to_write_aligned = (to_write + 511) & !511;
```

当 buf.len() 与 remaining 的差值不是 512 的倍数时，to_write_aligned 可能大于 remaining，于是向文件写入超出 1 GiB 的数据。session.rs:327-331 因为 actual 被 min 截断成 to_write，file_offset 不会超过 GIGABYTE，但文件物理长度会超过 1 GiB。

上游 f3write.c:163 `assert((turn_size & (SECTOR_SIZE - 1)) == 0)` 保证块大小恒为 512 整倍数。本项目 GUI 传入的 chunk_size_mb * 1024 * 1024 恰好是 512 整倍数，所以当前不触发。这是潜伏缺陷，任何把 chunk_size 改成非 512 整倍数的改动都会引爆它，并导致上游 f3read 的 block_order 计算错位。

修复方向：先 min(remaining) 再对齐：min((to_write + 511) & !511, remaining)。

---

### M-8 校验通过后自动清理只检查 stats 全绿，不检查本次 salt 是否可信

- 严重度：Medium
- 状态：已确认

代码位置：src/core/session.rs:613-629，auto_cleanup 且 bad == 0 且 overwritten == 0 且 changed == 0 时删除全部 .h2w。

触发条件：H-2 那种 salt 错配的情况下反而不清理（因为全红），所以真正的风险在另一种情形：salt 恰好正确但盘是假盘——不可能。真正的风险是：用户用正确 salt 写、用正确 salt 校验，但因为 M-2 少校验了尾部，stats 全绿，于是 .h2w 被全部删除，用户失去事后复核的唯一物证。

后果：测试文件被删，无法事后用上游 f3read 交叉复核。

修复方向：清理前要求覆盖范围完整（写入字节数等于校验字节数），并在报告里记录被删除的文件清单与哈希。

---

### L-1 校验读到的尾部不足 512 字节的数据会被静默丢弃

- 严重度：Low
- 状态：已确认但基本不可触发

代码位置：src/core/session.rs:515。FILE_FLAG_NO_BUFFERING 下读长度本身要求 512 整倍数，Windows 在末尾返回 ERROR_HANDLE_EOF，direct_io.rs:222-226 翻译为 Ok(0)，所以实际很难读到非 512 整倍数。

修复方向：无（被 Win32 语义掩盖）。

---

### L-2 discover_h2w_files 的文件名解析比上游严格

- 严重度：Low
- 状态：已确认

代码位置：src/core/session.rs:143-151 用 to_str()（非 UTF-8 文件名返回 None 直接忽略）与 parse::<u64>()（超大数失败）。上游 libfile.c:84-99 number_from_filename 逐字符累加，无溢出检查。

后果：本工具忽略超出 u64 的数字文件名，上游会回绕。不影响真实使用。

修复方向：无需处理。

---

### L-3 中间帧的失配计数会跳变

- 严重度：Low
- 状态：推测（需实测）

代码位置：src/core/session.rs:563-583 只有在 tracker.sample 返回 Some 时才发送遥测；若某次 read_at 处理的块数不够一个采样周期，该批的 stats 增量不会进入任何一条遥测。最终 session.rs:631-649 的 Completed 包会带完整 stats，所以结论正确，只是中间帧计数跳变，UI 失配热力图可能出现空洞。

修复方向：无需处理（最终统计正确）。

---

## 3. 与三条绕过维度的对应

### A. 假盘固件绕过

| 编号 | 绕过手法 | 本项目现状 | 严重度 |
| :--- | :--- | :--- | :--- |
| A-1 | 缓存欺骗：数据留在主控 DRAM/SLC，读命中缓存 | NO_BUFFERING 只绕过 OS 缓存，读取路径无排空动作（M-4） | Medium |
| A-2 | 写满截断：真实容量设成略小于标称，写满发生在末尾 | 写满后以 Completed 上报，不区分覆盖范围（C-1） | Critical |
| A-3 | 末尾截断：让最后一个文件长度短一截 | 校验不检查文件长度，EOF 即正常结束（M-2） | Medium |
| A-4 | 可预测模式：salt 来自时间戳且明文写在存证里 | one_way_seed 无法代数反解，但 salt 可从报告直接读到（H-4） | High |
| A-5 | 写入返回伪成功，实际丢弃 | Ok 但 0 字节时循环空转（H-6） | High |
| A-6 | 卷序列号伪装命中旧 salt | 前缀匹配使旧 salt 被误用（H-2） | High |

### B. 移植兼容性

- salt == 0：完全兼容（1.2、1.3、1.5 已逐项确认）。
- salt != 0：与上游完全不兼容，且默认开启（H-5）。
- 文件命名：N.h2w 起始编号 1，与上游一致；0.h2w 处理不同（M-1）。
- 块大小：本项目以 1 MB 为 I/O 单位，但数据生成始终按 512 字节扇区，与上游 chunk 大小无关，不影响兼容。
- 末尾不满块：M-7（潜伏）与 M-2。
- 扇区大小 512/4096：H-1，当前未生效，不破坏兼容，但也谈不上支持。
- 文件数边界：闭区间语义与上游一致，显示错位见 M-3。

### C. 防呆失效

- 校验用的 salt 可能来自别的盘（H-2），把真盘判成假盘并自动生成一份司法鉴定报告。这是本报告里对用户最危险的一条。
- 写满即停但报完成（C-1），用户以为测了整盘。
- salt 注册表被截断后无法再校验（H-3）。
- 校验通过后 auto_cleanup 删光 .h2w（M-8），用户失去事后复核的唯一物证。

---

## 4. 修复优先级建议

1. 先堵误杀真盘的路径：删掉 salt.rs:177-182 的前缀模糊匹配，查不到 salt 就明确报错（H-2）。
2. 写满必须 flush，写入结束状态必须区分写满与写完，报告写明实际覆盖字节数（C-1）。
3. salt 改用 BCryptGenRandom，报告只写指纹不写明文（H-4）。
4. 默认 salt 改回 0，加盐开关旁明确标注与原版 f3 / H2testw 不兼容（H-5）。
5. WriteFile 返回 Ok 但 0 字节时立即判失败（H-6）。
6. salt.rs 的 save 改成原子替换（H-3）。
7. 校验前检查每个 .h2w 长度是 512 整倍数（M-2）。
8. 报告文案按 salt 是否为 0 分支输出（M-6）。
9. to_write_aligned 先 min 再对齐（M-7）。

---

## 5. 审计方法与一处更正

- 本项目行号来自逐行读取 src/core/algo.rs（282 行）、src/core/session.rs（724 行）、src/core/aligned_buf.rs（88 行）、src/core/salt.rs（244 行）、src/core/direct_io.rs（435 行），以及为验证 salt 流向补读的 src/gui/app.rs:1150-1256、src/core/report.rs:400-560。
- 上游行号来自逐行读取 .upstream_f3/src/f3write.c（410 行）、f3read.c（456 行）、libflow.c（576 行）、libfile.c（221 行），以及 libutils.c:200-330 与 libutils.h:1-40。
- 更正：libflow.c 里没有随机数生成函数，任务是按 libflow.c 的 fill_buffer 去找，但真正的实现在 libutils.c:219-296（next_random_number、fill_buffer_with_block、validate_buffer_with_block）。libflow.c 只负责限速与测量。这一处更正本身值得记录，因为它是本次审计找到的第一个文档与代码不符的点。
- 未编译、未运行任何代码，全部结论基于静态阅读与手算推演；标注推测的条目已明确列出触发前提。
