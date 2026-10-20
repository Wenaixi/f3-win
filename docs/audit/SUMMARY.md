# F3-Win 对比上游 f3 的绕过风险总审计

审计对象：D:/newC/stick3/f3-win（commit 19cd53c）
对照基准：.upstream_f3/ 下 AltraMayor/f3 的 C 源码
审计方式：纯静态阅读与算术复现，未编译、未修改任何源码。

详细证据：
- docs/audit/00-lead-findings.md：Lead 亲验条目与对子代理结论的复核记录
- docs/audit/01-algo-compat.md：算法逐比特兼容性
- docs/audit/02-probe.md：探针状态机（含另一路子代理的 7 条与 18 条，已合并）
- docs/audit/03-platform.md：防呆、破坏性操作与 CI 门禁

---

## 零、一句话结论

算法内核是可靠的：salt == 0 时扇区数据与上游 f3 逐比特等价，四态判定逐分支一致，没有 SIMD 但也确实不需要靠它才能正确。问题全部集中在算法外围：取证用的盐策略会误杀真盘，探测算法会高估容量并破坏用户数据，GUI 的卷选择器没有任何安全门禁。

本次审计过程中，有两条被提交上来的 Critical 经复核不成立（详见第四节），已排除，不列入下面的清单。

---

## 一、最该先修的五条

按「真实伤害概率 x 后果」排序。

### 1. 逻辑卷选择器没有安全门禁，C 盘可以被写满

src/gui/components/device_selector.rs:95-97 的卷选择器直接 select_volume(idx)，没有任何 is_safe 判断；对比同一文件 :57 的物理盘选择器有 && is_safe。
src/gui/app.rs:1163 直接取 selected_path() 作为写入目标，此后无校验。

用户在下拉框选中 C 盘并点开始写入，session.rs 会一直写到 ERROR_DISK_FULL，即系统盘剩余空间全部被 .h2w 文件占满。不需要假盘、不需要恶意固件、不需要异常硬件。

修复：select_volume 前必须校验 is_safe_target，或让 start_h2w_task 在入口处统一拒绝系统盘。

### 2. 盐注册表按盘符前缀模糊匹配，正品盘被判成假盘

src/core/salt.rs:170-182。letter_key 只有单个字母（如 G），records 中 4 条 G:\ 记录会恒返回字典序最小的那条。
src/gui/app.rs:1183-1189 校验模式无条件调用 query_salt。
src/core/session.rs:522 用错误 salt 推导期望值，整盘判 Bad。
src/gui/app.rs:152-168 检测到 bad > 0 自动生成司法鉴定报告。

仓库根目录 f3_salt_registry.txt 实测 5 条记录中 4 条路径为 G:\，盐各不相同，说明多块盘先后插同一盘符，串味必现。

修复：删除 :177-182 与 :173-175，只保留 volume_serial 精确匹配；匹配失败时明确提示而不是猜。

### 3. 探测会把真实容量高估到离谱，用户按此分区会丢数据

src/core/probe.rs:200-208 的 scratch_window 只按逻辑 LBA 计算窗口，对物理容量一无所知。
src/core/probe.rs:484-532 的二分只在中点单测，中点通过即推左端点，不保留失败证据。

Lead 用真实常量复现（真实 33 MiB、标称 64 GiB、flushDepth 16384）：采样点 t=73600 已越过真实边界 67584，但其物理落点 6016 恰好使窗口 [4224, 6016) 完整落在真实容量内，于是写成功并通过校验，被当作安全点。Phase1 的 safe_left 因此被推到远超真实容量的位置，Phase2 在纯越界区间内搜索。

对照上游 libprobe.c:639-732 的 find_wrap 代数定位与 :645-662 的 ceiling_log2 精确覆盖。

### 4. 探测会真的破坏用户数据，且无还原

src/core/probe.rs:221-227 向 LBA 2048 写 512 字节高熵数据。1 MB 对齐布局下 LBA 2048 正是首分区起始扇区（NTFS 引导区）。
src/core/probe.rs:342-348 初始冲刷写 [4224, 6272)，会覆盖首分区开头的用户数据。
probe.rs 全文无 save/restore。

对照上游：libdevs.c:1233-1244 的 safe_device 默认备份还原，f3probe.c:470-481；只有加 --destructive 才不还原。

### 5. 写满不 flush 却报完成

src/core/session.rs:350-367 磁盘已满分支只在 file_offset > 0 时 flush，:361 的 flush 返回值被 let _ 丢弃。
src/core/session.rs:390-395 只满 1 GiB 才 flush。
src/core/session.rs:404-420 无论是否 file_full 都下发 Completed。
src/core/direct_io.rs:274-282 Drop 只 CloseHandle。

对照上游 f3write.c:233 每 chunk fdatasync 并检查返回，:234-237 首次错误优先保留。

假盘把真实容量设成略小于标称，即可让写入在末尾 ENOSPC，工具报「完成」，用户以为通过了。

---

## 二、按「假盘会不会骗过去」分类的完整清单

### A 类：假盘能骗过测试（漏报）

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| A1 | 写满不 flush 却报完成 | session.rs:350-367 / 390-395 / 404-420 |
| A2 | 探针扇区可一步反解会话盐 | probe.rs:105-123 |
| A3 | 基准探针 LBA 2048/4096 全程不被冲刷 | probe.rs:200-208（下界恒 4224） |
| A4 | 二分逼近漏判并高报容量 | probe.rs:484-532 |
| A5 | 排空量只有 1 MB ~ 8 MB | probe.rs:44-48、:326 |
| A6 | 读回只做单扇区单次读，紧跟写之后顺序读命中缓存 | probe.rs:229-238 |
| A7 | Phase2 通过后不清 trap，输出自相矛盾 | probe.rs:416-421、:515-517、:553 死代码 |
| A8 | 阶梯封顶 128 GiB，其上到盘尾无采样 | probe.rs:16-38、:437-478 |

A2 的细节：probe.rs:108 的 k = (sector_num ^ session_salt ^ pattern_salt) * 0xD6E8_FEB86659FD93，该常数为奇数，模 2^64 可逆；:109 的 x ^ (x >> 31) 与 wrapping_add 均是可逆仿射；:114 与 :115 把 sector_num 与 check_val 都明文写入。固件读到前 16 字节即可解出 session_salt ^ pattern_salt，而 pattern_salt 只有 0x55/0x66/0xAA/0xBB 四个值（:243/:247/:407/:457/:515），试四遍即得 session_salt，随后 :118 起的白噪声流完全确定。

上游 libutils.c:234-236 有明确注释警告不要这样做：Drives know the offset, so applying salt to it leaks the salt。

### B 类：正品盘被判成假盘（误报）

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| B1 | 盐注册表前缀模糊匹配串味 | salt.rs:177-182 |
| B2 | 4Kn 盘按 512 写，Error 87 被吞成 BitBucket | probe.rs:222 + direct_io.rs:99、:140 无调用点 |
| B3 | 写探针 / flush 失败一律判 BitBucket | probe.rs:222-227 |
| B4 | 盐注册表非原子写入，崩溃即全丢 | salt.rs:79-105 |
| B5 | WriteFile 返回 0 字节导致死循环 | session.rs:327-331 + :314 |
| B6 | 改 salt 后校验旧 .h2w 全盘 Bad 且无提示 | app.rs:1185、algo.rs:129-145 |

B6 补充：salt.rs:155 的 insert 会覆盖同卷旧记录，旧盐不可恢复；上游 f3write.c:123 / f3read.c:135 写死 salt = 0，根本没有这个维度。

### C 类：破坏用户数据

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| C1 | 写 C 盘（见第一节第 1 条） | device_selector.rs:95-97、app.rs:1163 |
| C2 | 探测写首分区起始扇区与首分区开头 | probe.rs:221-227、:342-348 |
| C3 | 全程无备份还原 | 对照 libdevs.c:1233-1244 |
| C4 | 取消后仍删光全部 .h2w | session.rs:508-510、:613 |
| C5 | diskpart 脚本的磁盘号在生成时冻结，rescan 可能改变编号 | fix.rs:176-199、:182 |
| C6 | diskpart 临时脚本落在公共 Temp 且名字可预测 | fix.rs:47-59、:70-71 |

### D 类：防呆失效

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| D1 | 系统盘反查失败时 fail-open，退化为只保护 index 0 | devices.rs:232-233、:455-462 |
| D2 | 用固件自报的 RemovableMedia 绕过总线白名单 | devices.rs:224、:233 |
| D3 | 依赖可被覆盖的 SystemDrive 环境变量 | devices.rs:456 |
| D4 | 卷锁守卫与直通句柄分离，空守卫仍可裸写 | devices.rs:410-417、probe.rs:260 |
| D5 | diskpart 的 verify_safety 只在生成脚本前跑，spawn 时不复查 | fix.rs:274-282 |

D1 与 D2 叠加：非 0 号的内置固定盘可同时绕过两道防线。

### E 类：盐策略

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| E1 | 默认开盐，与原版 f3/H2testw 不互通 | state.rs:71 + algo.rs:55-80 |
| E2 | salt 熵低（SystemTime::as_nanos） | app.rs:1170-1173、:1205-1208 |
| E3 | salt 明文写进对外报告 | report.rs:406 |
| E4 | salt = 0 时报告仍宣称「信息论上无法逆解」 | report.rs:408 |

### F 类：CI 门禁不阻断

| 编号 | 问题 | 位置 |
| --- | --- | --- |
| F1 | clippy 缺 -- -D warnings | ci.yml:26-27、release.yml:25-26 |
| F2 | cargo test 缺 --all-targets 与 --include-ignored | ci.yml:29-30、release.yml:28-29 |
| F3 | aarch64 只编译不测试 | ci.yml:50-51 |

---

## 三、文档与代码不符清单

逐条核实过：

1. 「AVX2 / SSE4.2 / NEON 向量化 + 硬件 popcount」：src/ 下 grep simd/avx/sse/_mm/target_feature/std::arch 零命中。algo.rs:120-138 是标量循环，且因内部有 LCG 递推与提前 return，LLVM 无法自动向量化。
2. 「512B / 4096B (4Kn) 动态自适应，杜绝 Error 87」：direct_io.rs:140 的 set_sector_size 全仓无调用点，sector_size 在 :79/:105/:130 硬编码 512。且 session.rs:321 写死 511 与 direct_io.rs:167-172 按 sector_size 断言矛盾，4Kn 一旦接通会 assert panic。
3. 「排空 32MB ~ 64MB」：probe.rs:44-48 上限 16384 扇区 = 8 MB，初始排空 2048 扇区 = 1 MB。上游 find_cache_size 实测，上限 1 GiB。
4. 「单向不可逆 PRF，Salt 永不落盘，信息论上无法反解」：探针路径用的是可逆仿射（见 A2）；报告路径把 salt 明文写出（见 E3）。
5. 「默认 Salt = 0，与 H2testw / 原版 F3 百分百相互校验」：state.rs:71 默认 use_random_salt = true，默认即不互通。
6. 「f3probe 为非破坏性探测」：实际覆盖首分区引导区与开头数据，无还原（见 C2、C3）。

---

## 四、两条被复核推翻的子代理结论（记录以免误采信）

### 误报一：乘法器 4294967311 与上游不等价

子代理主张 4294967311 同余于 -1 (mod 2^16)，上游运算是 17 - 2^16 * X，本项目 wrapping_mul 是 mod 2^64。

实际：4294967311 mod 65536 = 15，不是 65535。且 C 标准的 uint64_t 无符号溢出按 mod 2^64 回绕，是标准明确定义的行为，不会产生 17 - 2^16 * X。Rust wrapping_mul 同为 mod 2^64。Lead 用 0x1 / 0x3039 / 0xDEADBEEF / 0xFFFFFFFFFFFFFFFF 四组数值反证，结果逐位相同。

### 误报二：block_order 语义不一致导致整盘布局错位

子代理主张上游一个块只用一条 PRNG 流，本项目每 512 字节重新播种，默认 1 MB 块时布局必然错位。

实际：上游 .upstream_f3/src/f3write.c:119-127 的 fill_buffer 显式传 SECTOR_ORDER，且每个 512 字节扇区用递增后的 poffset 重新播种：

  for (i = 0; i < sectors; i++) {
      fill_buffer_with_block(buf, SECTOR_ORDER, *poffset, 0);
      buf += SECTOR_SIZE;
      *poffset += SECTOR_SIZE;
  }

block_order 在文件写入路径上只用于 f3write.c:186 计算 total_file_blocks 和控制写入节奏，不参与内容生成。本项目 algo.rs:84-98 的逐扇区播种与之完全一致。子代理的算术没错，但前提假设错了。

---

## 五、建议修复顺序

第一梯队（改动小、收益大，建议立刻做）

1. device_selector.rs 的卷选择器加 is_safe 门禁，start_h2w_task 入口统一拒绝系统盘。
2. 删除 salt.rs:177-182 的模糊匹配与 :173-175 的单字母兜底，只保留 volume_serial 精确匹配。
3. devices.rs 系统盘反查失败时 fail-closed；移除 :233 的 is_removable 旁路；改用 GetSystemDirectoryW 替代环境变量。
4. session.rs 所有退出路径都 flush 并检查返回值；写满不报 Completed；written == 0 立即失败。
5. 写探针 / flush 失败不再判 BitBucket，改为明确报 I/O 异常。

第二梯队（需要设计决策）

6. 盐策略三选一并写进文档：关闭随机盐保证互通；保留但明确标注不互通；或独立 salt 文件 + 报告只写指纹。
7. 探针块重构：去掉明文 LBA，或确保 session_salt 不可解。
8. 探测备份还原：至少对 LBA 2048 与冲刷区先读后写、结束写回，或改用强制二次确认。
9. fix.rs 临时脚本改到用户私有目录并加独占创建；verify_safety 移到 spawn 之前复查。

第三梯队（算法级重做）

10. 重写 probe 的区间搜索，采用上游 libprobe.c:639-732 find_wrap 与 :645-662 ceiling_log2 的形态。
11. 排空量改为实测（参考 libprobe.c:568-637 find_cache_size）。
12. 4Kn 要么真正实现（注意上游种子只依赖全局 offset），要么从文档删除该宣称。

CI 侧

13. clippy 加 -- -D warnings；cargo test 加 --all-targets；移除或登记所有 #[ignore]。

文档侧

14. 上述 6 处不符，要么改文档要么改代码。

---

## 六、本次审计未覆盖的部分

- 未做动态测试，全部结论来自静态阅读与算术复现。受项目「严禁本地编译」铁律约束。
- 未审计 GUI 渲染、系统托盘、路径服务等非安全路径模块。
- 未审计 report.rs 中除 salt 相关之外的文案正确性。
- 未对 build.rs 与 Cargo.toml 依赖链做供应链审计。
- docs/audit/02-probe.md 内合并了两路子代理的发现，条目编号有重叠，以本文件第一节与第二节为准。
