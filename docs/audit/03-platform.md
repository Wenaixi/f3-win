# F3-Win 审计报告 03：防呆、破坏性操作与 CI 门禁

审计对象：D:/newC/stick3/f3-win（Rust 移植版 F3，Windows）
上游对照：D:/newC/stick3/f3-win/.upstream_f3/src/
审计方式：纯静态阅读（read / grep），未执行任何编译，未修改任何源码
发现条数：22（第十四至二十二条由第二轮复查补入，并在第五条补充了与上游恢复语义的差异）

---

### 1. NVMe 系统盘在 Windows 上 RemovableMedia 报告为真，可绕过总线白名单直达物理写（Critical）

**现象**
物理盘安全判定依赖 STORAGE_DEVICE_DESCRIPTOR.RemovableMedia，该字段在 NVMe 协议下没有语义，Windows 的 stornvme.sys 一律报告 TRUE。因此内置 NVMe 系统盘必然满足 is_removable，放行条件退化为只看磁盘编号。

**证据(文件:行号)**
- src/core/devices.rs:224  `let is_removable = desc.RemovableMedia.as_bool();` 直接采信该字段
- src/core/devices.rs:232-233
```rust
let is_sys_disk = index == 0 || sys_disk_idx == Some(index);
let is_safe = (bus_type.is_removable_bus() || is_removable) && !is_sys_disk;
```
放行条件是 ||：总线为 SATA/NVMe/Unknown 时，只要 is_removable 为真就整个放行。
- src/core/devices.rs:53-55  `is_removable_bus()` = `matches!(self, Usb | Sd | Mmc)`，Nvme 本不放行，被 `|| is_removable` 短路覆盖
- src/core/devices.rs:358  `self.device.is_safe_target && !is_sys_disk && !contains_system_drive`
- src/core/devices.rs:369-374  prepare_for_probe 的现场复核读的是 live_dev.is_safe_target，与首次枚举同源，无法发现该问题
- src/core/devices.rs:461-462  系统盘磁盘号唯一来源 get_system_drive_disk_number() -> get_physical_drive_number_for_volume('C')
- src/core/devices.rs:473  该函数以访问权限 0 打开卷并下发 IOCTL_STORAGE_GET_DEVICE_NUMBER，任何一步失败返回 None（devices.rs:504）

**上游差异(文件:行号)**
- .upstream_f3/src/f3fix.c:279  `dev = ped_device_get(args.dev_filename);` 目标设备完全由用户命令行显式给出，工具不枚举、不自动选盘
- .upstream_f3/src/f3probe.c:443-444  上游只打印 WARNING，从不声称做过总线白名单隔离
- 上游没有自动选盘逻辑，因此不存在自动选盘误选系统盘的面；本项目的自动枚举与自动选优（devices.rs:733-745）是上游没有的新攻击面

**攻击者如何利用**
1. NVMe 系统盘 is_removable 恒真，is_safe 为真（只要磁盘号不是 0）。
2. 系统盘不是 PhysicalDrive0（多盘机、NVMe 非首个槽位、引导卷挂在非 0 号）时 is_sys_disk 为假。
3. 系统盘未挂盘符时 get_physical_drive_number_for_volume('C') 返回 None，is_sys_disk 与 contains_system_drive 双双失效。
4. 上述任一路径成立，该盘在设备下拉框显示为可选；用户点开始探测即在系统盘执行 devices.rs:410 的 open_block_io 与 probe.rs:305、probe.rs:312 的裸扇区写入。

**一句话修复方向**
放行条件改为 `bus_type.is_removable_bus() && !is_sys_disk`，或规定 is_removable 只能作为 USB 总线的补充判据、禁止为非 USB 总线单独放行。

---

### 2. 卷独占锁守卫与写句柄不互斥，未真正持锁也可写入裸扇区（Critical）

**现象**
open_block_io 先拿守卫再打开物理设备，两者是两个互不感知的返回值。VolumeLockGuard 只断言 Send/Sync，不提供任何互斥语义；分卷枚举失败时 mounted_volumes 为空，循环零次迭代照样返回 Ok。F3Probe::run 的签名里没有任何锁凭据。

**证据(文件:行号)**
- src/core/devices.rs:70-76  VolumeLockGuard { handles, locked_volumes }，unsafe impl Send/Sync（行 75-76）
- src/core/devices.rs:386-400  逐个卷 lock_and_dismount_volume；失败分支回滚并 return Err（行 392-397），但行 400 循环零次迭代时同样直接返回 Ok，不校验 locked_volumes 是否等于 mounted_volumes
- src/core/devices.rs:163-170  卷反查 `if let Some(disk_num)` 不匹配时静默跳过，导致 mounted_volumes 可能为空
- src/core/devices.rs:541-606  VolumeInfo::enumerate_volumes 只认盘符，未挂盘符的分区完全不在 mounted_volumes 里
- src/core/devices.rs:410-417
```rust
pub fn open_block_io(&self) -> Result<(DirectIoFile, VolumeLockGuard), String> {
    let guard = self.prepare_for_probe()?;
    let path = std::path::Path::new(&self.device.path);
    match DirectIoFile::open_raw_device(path) { ... }
```
- src/core/controller.rs:187  `let (mut block_file, lock_guard) = target.open_block_io()?;`
- src/core/controller.rs:207-209
```rust
let _guard = lock_guard;
let probe = F3Probe::new(config, flag);
let _ = probe.run(&mut block_file, tx);
```
- src/core/probe.rs:260  `pub fn run(&self, dev: &mut impl BlockIo, tx: Sender<ProbeTelemetry>) -> Result<ProbeResult, String>` 参数里没有任何锁凭据
- src/core/devices.rs:265-274  lock_and_dismount_volume 用 FILE_SHARE_READ|FILE_SHARE_WRITE 打开卷，共享读写本身不提供排他性

**上游差异(文件:行号)**
- .upstream_f3/src/f3probe.c:428-440  args->save 为真时用 create_safe_device 包装设备，写前备份
- .upstream_f3/src/f3probe.c:470-474  `if (sdev) {... printf("Probe finished, recovering blocks..."); }` 探测结束强制恢复
- .upstream_f3/src/f3probe.c:49-50  `{"destructive", 'n', ..., "Do not restore blocks of the device after probing it"}` 不恢复必须显式选择
- 上游语义是先备份再写、写完必恢复；本项目 probe.rs 全文无备份与恢复路径

**攻击者如何利用**
这是用户操作变成不可逆：
1. 卷反查失败使 mounted_volumes 为空，空守卫下仍对 LBA 2048 与 LBA 4096 写裸扇区（probe.rs:305、probe.rs:312）。
2. 卡上存在未挂盘符的分区时，该分区的文件系统缓存从未卸载，却被底层直写覆盖。
3. FSCTL_LOCK_VOLUME 被杀软或索引服务抢占而三次重试全失败（devices.rs:285-310）时，若该卷不在 mounted_volumes 里则根本不会被尝试。
4. 结果：用户写坏自己的文件系统，无备份可恢复，UI 仍显示设备已安全解锁恢复挂载（app.rs:545）。

**一句话修复方向**
让 open_block_io 强制校验 guard.locked_volumes 与此磁盘全部已挂载卷一一对应，并把 F3Probe::run 的签名加上 &VolumeLockGuard 作为类型级前置条件，同时补上 create_safe_device 等价的写前备份。

---

### 3. diskpart 脚本含范围过大的 clean，且磁盘号在脚本生成时冻结，无现场复核（Critical）

**现象**
脚本是 `select disk N` 紧接 `clean`，模板是死字符串，磁盘号在 F3FixPlan 构造时即冻结。verify_safety 的现场复核发生在生成脚本之前，rescan 又放在 select disk 之前，磁盘编号重排即抹错盘。

**证据(文件:行号)**
- src/core/fix.rs:176-199  模板原文（占位符顺序：announced_bytes, announced_gb, real_bytes, real_gb, safe_partition_mb, disk_index, safe_partition_mb, file_system）
```
REM F3-Win 自动生成的安全分区修复脚本
REM 原始标称容量: {} 字节 ({:.2} GB)
REM 真实物理容量: {} 字节 ({:.2} GB)
REM 安全截断分区: {} MB
rescan
select disk {}
attributes disk clear readonly
clean
convert mbr
create partition primary size={}
format fs={} quick
assign
```
- src/core/fix.rs:185  `clean` 抹掉磁盘上所有分区与全盘前 1MB 及后部 GPT 备份，作用范围是整个磁盘
- src/core/fix.rs:182  `rescan` 位于 select disk 之前，会重新枚举磁盘，编号可能变化
- src/core/fix.rs:274-282  verify_safety 里唯一一次现场复核 PhysicalDeviceInfo::query_by_index(self.plan.disk_index)，发生在 run_script（fix.rs:303-306）之前，到 spawn diskpart.exe（fix.rs:70-71）时已不再检查
- src/core/fix.rs:216-225  execute_with_runner 对 disk_index 无任何范围校验，u32 上界完全放开
- src/core/fix.rs:371 与 fix.rs:428  单测 `script.contains("clean")` 把 clean 写成期望行为

**上游差异(文件:行号)**
- .upstream_f3/src/f3fix.c:203-231  ped_disk_new_fresh -> ped_partition_new -> ped_disk_add_partition -> ped_disk_commit，在同一个已打开设备句柄上提交分区表，不存在按数字编号让外部进程重新定位磁盘的间接层
- .upstream_f3/src/f3fix.c:252-253  `/* Skip first 1MB to avoid the partition table. */ .first_sec = MEGABYTE_SIZE / SECTOR_SIZE`，只调整分区表，不做全盘 clean
- .upstream_f3/src/f3fix.c:283-285  目标设备由 args.dev_filename 显式传入，不经过数字编号

**攻击者如何利用**
1. 用户先点开始探测拿到修复方案（app.rs:608 的 start_fix）。
2. 探测完成到点应用安全分区之间，磁盘枚举顺序被改变：USB 集线器多插一个盘、虚拟机热插拔，或脚本自身的 rescan（fix.rs:182）。
3. controller.rs:219 的 verify_safety 在点击瞬间复核通过，controller.rs:229-232 spawn 线程执行 fixer.apply()。
4. diskpart.exe 启动后、clean 生效前磁盘号再次变化时，clean 落到另一个占用该编号的磁盘，该盘任意用户数据被全盘抹除。
5. 即使编号无误，clean 也是无差别全盘操作，多分区卡上其它分区数据一并丢失。

**一句话修复方向**
把 select disk N 换成 select volume <盘符>，或 select disk 后紧跟 detail disk 并用磁盘序列号做二次断言，或改用 IOCTL_DISK_SET_DRIVE_LAYOUT_EX 在已持有的物理句柄上直接写分区表。

---

### 4. 临时 diskpart 脚本名可预测、落在公共 Temp、非独占创建，存在抢注劫持（High）

**现象**
脚本路径由 temp_dir() 加 f3_fix_{pid}_{millis}.diskpart 拼成，随后 std::fs::write 直接覆盖写。PID 与毫秒时间戳都是低熵可枚举值，写到执行之间没有任何独占性保证。

**证据(文件:行号)**
- src/core/fix.rs:47-56
```rust
let temp_dir = std::env::temp_dir();
let file_name = format!(
    "f3_fix_{}_{}.diskpart",
    std::process::id(),
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis()).unwrap_or(0)
);
let script_path = temp_dir.join(file_name);
```
- src/core/fix.rs:58-59  `std::fs::write(&script_path, script)` 语义为 CREATE_ALWAYS，静默截断并覆盖同名文件；若被占位为重解析点则写入被重定向
- src/core/fix.rs:70-71  `Command::new("diskpart.exe").arg("/s").arg(&script_path);` 执行前不再校验路径身份
- src/core/fix.rs:62-68  TempCleaner 只在 Drop 时 remove_file，无法检测文件被替换
- src/gui/app.rs:621-623  另一个特权临时产物导出到 USERPROFILE/Desktop/f3_fix.diskpart，固定文件名每次覆盖

**上游差异(文件:行号)**
- .upstream_f3/src/f3fix.c:241-287  main 全程在进程内用 libparted 完成，不生成任何临时脚本文件
- 该临时文件面完全由移植层引入

**攻击者如何利用**
1. 攻击者以当前用户身份高频轮询 %TEMP%（通常 C:/Users/<user>/AppData/Local/Temp，同用户可读写）。
2. 出现 f3_fix_*.diskpart 时，在 F3-Win 写完、diskpart 读取之前替换内容为任意指令（例如 select disk 0 加 clean）。
3. F3-Win 以管理员权限启动的 diskpart.exe /s 原样执行被替换脚本，完成提权或擦除任意磁盘。
4. 该路径不需要管理员即可写，而 F3-Win 以管理员运行，构成低权限写、高权限执行的经典 TOCTOU。

**一句话修复方向**
改用 CreateFileW 加 CREATE_NEW 独占创建临时脚本（失败即换名重试），spawn 前用 GetFinalPathNameByHandle 校验路径指向的是刚创建的对象，或把脚本经 diskpart 标准输入喂入而不落盘。

---

### 5. f3probe 无备份覆盖 LBA 2048 与 LBA 4096，取消或失败后原数据永久丢失（High）

**现象**
探测把破坏性写入当默认行为，不区分 --destructive。两个探针位置恰好是 Windows 上 create partition primary 默认的 1MB 对齐分区起点，多数 U 盘上这两个扇区就是第一个分区的引导扇区与 FAT 表开头。

**证据(文件:行号)**
- src/core/probe.rs:9-10
```rust
pub const BASE_PROBE_SECTOR: u64 = 2048; // 1MB 偏移处主基准探针
pub const AUX_PROBE_SECTOR: u64 = 4096;  // 2MB 偏移处辅助基准探针
```
- src/core/probe.rs:305-317  create_probe_block 后立刻 dev.write_at 到 BASE_PROBE_SECTOR * 512 与 AUX_PROBE_SECTOR * 512，每处 512 字节，写入前无读取、无保存
- src/core/probe.rs:319-323  写完立即 dev.flush()，物理落盘不可回滚
- src/core/probe.rs:260  run 只有 dev 一个设备参数，函数体内不存在原始内容缓冲；probe.rs 全文无 save/restore/备份 标识符
- src/gui/app.rs:519  UI 提示测试前会自动卸载卷请备份重要数据，但卸载卷不保存第一个分区的前 2MB

**上游差异(文件:行号)**
- .upstream_f3/src/f3probe.c:571  `.save = true,` 默认开启保底恢复
- .upstream_f3/src/f3probe.c:428-440  `if (args->save) { sdev = create_safe_device(dev, probe_max_written_blocks(dev), args->min_mem); ... dev = sdev; }`
- .upstream_f3/src/f3probe.c:470-474  `if (sdev) { ... printf("Probe finished, recovering blocks..."); }`
- .upstream_f3/src/f3probe.c:49-50  只有显式 --destructive 才跳过恢复

**攻击者如何利用**
误操作即数据销毁：对一盘装有机密资料但被误认为空盘的 U 盘执行探测，LBA 2048/4096 被写入伪随机探针，分区表残留但 FAT/exFAT 目录区或位图起始字节被改写，挂载后表现为目录乱码或直接要求格式化。用户在 app.rs:530 点停止探测时，cancel 只置位标志（controller.rs:143-146），两个探针扇区已 flush 落盘（probe.rs:319-323），无法撤销。

**第二轮复查补充：备份粒度差异（上游同样不代表无损）**
- .upstream_f3/src/f3probe.c:428-434  create_safe_device(dev, probe_max_written_blocks(dev), min_mem) 的块数由 probe_max_written_blocks 决定；libdevs.c:1115-1336 的 sdev 只对真正写过的块做写前保存，sdev_recover 逐块写回。
- 上游同样会写 LBA 2048 起点之后的数据，只是会恢复"被写过的那几块"。若探测中途掉盘、进程被强杀、USB 复位失败（libdevs.c:649-730），上游同样恢复不了。
- 本项目的差异在于连"写过就备份"这一层都没有，因此在正常完成的路径上，上游无损、本项目有损；这是移植时被丢掉的一整块语义，而不是同一语义下的参数差异。

**一句话修复方向**
在 probe.rs:306 与 probe.rs:313 前先读取这两个扇区的原始 512 字节，在 run 的全部返回路径（含错误与取消）末尾写回并 flush，与上游 create_safe_device 语义对齐。

---

### 6. 卷选择器无任何安全过滤，C 盘可直接被选为写入目标（Critical）

**现象**
物理盘下拉框有 is_safe 门禁，卷下拉框没有。start_write_task 与 start_read_task 直接取 inventory.selected_path()，从不校验该路径所在卷是否为系统盘或内部固定磁盘。写入流程会以 Direct I/O 把分区填到剩余空间耗尽。

**证据(文件:行号)**
- src/gui/components/device_selector.rs:90-98
```rust
for (idx, vol) in volumes.iter().enumerate() {
    let resp = ui.selectable_label(selected_idx == idx, vol.display_name(),);
    if resp.clicked() {
        inventory.select_volume(idx);
    }
}
```
对比同文件行 57：`if resp.clicked() && is_safe { inventory.select_device(idx); }` 物理盘有门禁、卷没有
- src/core/devices.rs:669-673  select_volume 只检查 index < self.volumes.len()
- src/gui/app.rs:1162-1163  `fn start_h2w_task(&mut self, mode: H2wMode) { let target_dir = self.state.inventory.selected_path();`
- src/gui/app.rs:1219-1230  直接构造 H2wConfig { target_dir, ... } 并 start_h2w
- src/gui/app.rs:262-263  `if ui.button(RichText::new("开始写入")).clicked() { self.start_write_task(); }` 无二次确认、无系统盘拦截
- src/core/session.rs:281-288  max_bytes 可选，为空时 target_dir.join(format!("{}.h2w", file_index + 1)) 一直写到 ERROR_DISK_FULL（session.rs:351-357）
- src/gui/state.rs:73  `chunk_size_mb: 1`
- src/core/session.rs:160-172  cleanup_h2w_files 遍历 discover_h2w_files（session.rs:140-157）全部 remove_file

**上游差异(文件:行号)**
- .upstream_f3/src/f3write.c:219  `offset = number << GIGABYTE_ORDER;` 上游以逻辑容器号定位
- .upstream_f3/src/f3write.c:233  每次分块写后立即 fdatasync，写满会立刻暴露
- 上游无 GUI，不存在下拉框里 C: 与 U 盘并排造成的误操作面

**攻击者如何利用**
- 社工场景：用户手滑选中 C: 后点开始写入，系统盘剩余空间被填满，Windows 进入低磁盘状态无法登录、更新失败、页面文件无法扩展。
- session.rs:351-355 把 ERROR_DISK_FULL 当作正常写满标记 file_full = true 并继续（行 357-367），UI 显示为正常完成，用户不会意识到填满了系统盘。
- 清理同样无门禁：app.rs:334-343 与 app.rs:449-461 的清理测试文件直接对 selected_path() 执行 start_cleanup，会删掉 C 盘根目录下所有匹配 <数字>.h2w 的文件。

**一句话修复方向**
在 start_h2w_task 与 start_cleanup 入口统一校验 target_dir 所在卷既不是 %SystemDrive% 也不属于任何 is_safe_target == false 的物理盘，并在 device_selector.rs:95 与物理盘同门禁。

---

### 7. 自动清理不看取消标志，中途停止后仍删除全部 .h2w（Medium）

**现象**
auto_cleanup 的守卫只看三项统计是否为 0，不看校验是否真正跑完。取消标志只 break 内层读循环，外层继续下一个文件；读失败与取消都不增加 bad/overwritten/changed，统计全 0 时依然满足清理条件。

**证据(文件:行号)**
- src/core/session.rs:508-510
```rust
if self.cancel_flag.load(Ordering::Relaxed) {
    break;
}
```
只跳出当前文件的内层块循环，execute_verify 的外层 for 继续处理后续文件
- src/core/session.rs:613
```rust
if self.config.auto_cleanup && stats.bad == 0 && stats.overwritten == 0 && stats.changed == 0 {
```
条件里没有 cancel_flag、没有已完成标志、没有实际校验字节等于预期字节
- src/core/session.rs:628  `let _ = Self::cleanup_h2w_files(&self.config.target_dir);` 无差别删除该目录下所有合法命名的 .h2w
- src/gui/state.rs:77  `auto_cleanup_after_read: false` 默认关闭，用户一旦在 app.rs:464 勾选即长期生效
- src/gui/app.rs:1227  `auto_cleanup: self.state.auto_cleanup_after_read` start_h2w_task 不按 mode 分流，写入模式也带着这个开关

**上游差异(文件:行号)**
- 上游 f3read 与 f3write 从不自动删除文件，删除永远是用户显式操作
- 自动清理是本项目自创行为，其安全性完全依赖本项目守卫条件，而该守卫不覆盖取消与 I/O 失败

**攻击者如何利用**
1. 用户对假盘跑校验，中途因耗时长点停止校验（app.rs:407-408 调 controller.cancel()）。
2. 已校验的前若干 GB 未发现异常，bad/overwritten/changed 全 0。
3. session.rs:613 不看取消标志，cleanup_h2w_files 把包括未校验部分在内的所有 .h2w 全部删除。
4. 用户失去本应用的核心价值定位，即完整保留假盘维权证据（session.rs:612 的注释正是这么宣称的）。

**一句话修复方向**
在 session.rs:613 条件中追加 `&& !self.cancel_flag.load(Ordering::Relaxed) && verified_bytes == expected_bytes`，并把是否真正跑完全部文件作为独立布尔返回。

---

### 8. diskpart 退出码被丢弃，失败检测只靠中英文文本匹配（Medium）

**现象**
先 try_wait 拿到状态绑定到 let _status 后从未使用，随后又一次 wait_with_output 重新取输出；真正的成败判定依赖固定英文与中文子串，本地化版本 Windows 上只剩 status.success() 兜底。

**证据(文件:行号)**
- src/core/fix.rs:86  `let _status = loop {` 绑定后从未使用
- src/core/fix.rs:100  `let output = child.wait_with_output().map_err(...)?;`
- src/core/fix.rs:105-108
```rust
let has_err = stdout_str.contains("DiskPart has encountered an error")
    || stdout_str.contains("Virtual Disk Service error")
    || stdout_str.contains("错误")
    || !output.status.success();
```
- src/core/fix.rs:110-119  判定成功时直接把 stdout_str 当 Ok 返回，controller.rs:388-391 据此把状态置为 Finished 并显示分区修复成功

**上游差异(文件:行号)**
- .upstream_f3/src/f3fix.c:283-285  `ret = !fix_disk(...); printf("Drive `%s' was successfully fixed\n", args.dev_filename); ped_device_destroy(dev); return ret;` 返回值来自 fix_disk 的真实 bool，不解析文本

**失效方向经过修订（原稿写反了）**
真实的失效不是漏报失败，而是恒假成功：
1. windows 0.58 的 Command::output() 实现在 self.stdout.is_none() 时会把子进程输出重定向到匿名管道再读出（与 wait_with_output 同源实现）。try_wait 只收割退出状态，不关闭管道，因此 fix.rs:100 仍可能拿到完整 stdout。
2. diskpart.exe 的真实报错输出不带 "DiskPart has encountered an error" 这段英文前缀（该串是 Microsoft.Vhd.PowerShell / 脚本层的措辞），也不带 "Virtual Disk Service error"，中文系统上则不出现半角 "错误"。三条文本匹配在真实失败场景基本不可能命中。
3. diskpart.exe 在 "/s 脚本" 模式下即使某条命令失败，也常常以 0 退出（错误只写进 stdout 文本）。
4. 因此 has_err 恒为 false，fix.rs:110-119 返回 Ok，controller.rs:384-408 把 status 置 Finished 并显示分区修复成功，而磁盘实际仍是旧分区表。
5. 反向风险同时存在：中文系统上 "错误" 若出现在正常提示里（如 "未发现错误"），会把成功判成失败。

**上游差异(文件:行号)**
- .upstream_f3/src/f3fix.c:281-287  `ret = !fix_disk(dev, args.disk_type, args.fs_type, args.boot, args.first_sec, args.last_sec);` 成功后打印，`return ret;` 直接返回 fix_disk 的真实布尔，不解析任何文本

**一句话修复方向**
改用 `select disk N` 之后 `detail disk` 的输出做结构化断言（或读回 IOCTL_DISK_GET_DRIVE_LAYOUT_EX 校验分区表真的变成目标布局），并同时以 ExitStatus::code() != 0 作为失败判据之一。

---

### 9. CI 的 clippy 门禁不带 -D warnings，新引入的警告不阻断合并（High）

**现象**
两个 workflow 的 Clippy 步骤都是 cargo clippy --all-targets，没有加 -- -D warnings。clippy 默认以 warning 报告且退出码为 0，因此新引入的 warning 不会让 CI 变红。

**证据(文件:行号)**
- .github/workflows/ci.yml:26-27
```yaml
- name: Clippy Lint
  run: cargo clippy --all-targets
```
- .github/workflows/release.yml:25-26  完全相同的两行
- 两个文件均无 -D warnings、-D clippy::all、RUSTFLAGS 或 CARGO_ENCODED_RUSTFLAGS（grep 无命中）

**上游差异(文件:行号)**
- 上游 .upstream_f3/src/ 为 C 项目（f3write.c、libprobe.c 等），不使用 cargo，无对应 CI 门禁；此条是移植工程自身治理缺陷

**攻击者如何利用**
供应链与质量门禁失效：一次 #[allow] 失控、一次 unwrap（例如 fix.rs:329 的 lock().unwrap()）在 --all-targets 下产生的警告不阻断合并，真正危险的 lint（missing_panics_doc、cast_possible_truncation）与噪音混在一起后没人再看。

**一句话修复方向**
改为 `cargo clippy --all-targets -- -D warnings`，并补上 `cargo fmt --check`。

---

### 10. cargo test 未覆盖全部目标与架构，且无机制阻止 #[ignore] 静默跳过（High）

**现象**
cargo test --verbose 只在默认 host 上跑 lib/bin/tests，aarch64-pc-windows-msvc 目标只编译不测试；也没有任何步骤检查 #[ignore] 数量，新增 #[ignore] 可让测试静默跳过。

**证据(文件:行号)**
- .github/workflows/ci.yml:29-30
```yaml
- name: Run Core Unit Tests
  run: cargo test --verbose
```
- .github/workflows/release.yml:28-29  同上
- 两个 workflow 全文无 --target、无 --all-targets、无 cargo test --no-default-features、无 doctest 专用步骤
- .github/workflows/ci.yml:32-34  `build-release: needs: check-and-test`，矩阵构建立即开始，不对 --target aarch64 执行任何测试
- .github/workflows/ci.yml:50-51  `cargo build --release --target ${{ matrix.target }}` 只编译不测试
- 当前 src 下 grep ignore 零命中，但这是当前没有而非 CI 禁止，缺少防线

**上游差异(文件:行号)**
- 上游 .upstream_f3/src/ 全部为 C，无 cargo test 概念；此条同样属于移植工程自身治理缺陷

**攻击者如何利用**
门禁绕过：提交一个带 #[cfg(target_arch = "x86_64")] 门控的实现并配 #[ignore]，CI 依然全绿；aarch64 用户拿到从未被任何测试覆盖的二进制。CLAUDE.md 记录的 46 项单测全绿只在 windows-latest 默认 host（x86_64）上运行过。

**一句话修复方向**
改为 `cargo test --all-targets --verbose -- --include-ignored`（或显式断言 --ignored 列表为空），并在 build-release 矩阵中为 aarch64 增加 `cargo test --target ${{ matrix.target }} --no-run`。

---

## 附：已核查判定为合格的项（记录以免重复审）

- .github/workflows/ci.yml 与 release.yml 全文均无 continue-on-error（grep 零命中），不存在被静默吞掉的失败步骤。
- src/gui/app.rs:100-110 的关闭按钮拦截：minimize_to_tray_on_close 为真时先 CancelClose，运行中先 CancelClose 再弹模态，逻辑正确。
- src/core/controller.rs:139-146 的 cancel 只置 cancel_flag，不直接终止线程，RAII 守卫在 worker 线程内 Drop，路径正确（问题在于锁没有被类型系统强制，见第 2 条）。
- src/core/fix.rs:212-215 与 fix.rs:251-256 的 disk 0 与系统引导盘双重拦截是存在的，fix.rs:343-350 有单测覆盖。
- src/core/devices.rs:355 的 is_safe_for_raw_probe 三重判定（编号 0、反查编号、含系统卷盘符）本身逻辑完整，缺陷在于其输入 is_safe_target 的构造有第 1 条的问题。

---

## 第二轮复查补入（第 14 至 22 条）

以下条目来自对 app.rs 420-1299、session.rs、probe.rs、tray.rs、main.rs、device_selector.rs 的逐行复查。为不与前十三条重复，凡前文已覆盖的主题不再重列。

---

### 14. 校验中途取消被判为 Finished 且 error=None，未校验完的部分直接显示为通过（Critical）

**现象**
execute_verify 的取消判断分两层：文件级（session.rs:461-477）会发 Aborted 并 return false；内层读循环（session.rs:506-510）只 break 出循环，随后照常走到函数尾部发 Completed 且 error=None。controller.rs:298-310 见 is_finished 且 error 为空即判 Finished。app.rs:473 的报告卡渲染条件只要求 status==Finished 且 bytes>0，不区分是否跑完。

**证据(文件:行号)**
- src/core/session.rs:505-510
```rust
loop {
    if self.cancel_flag.load(Ordering::Relaxed) {
        break;            // 只跳出读循环，不 return
    }
```
- src/core/session.rs:631-649  函数尾部无条件发 `H2wPhase::Completed ... true, None, &tracker`（is_finished=true, error=None）
- src/core/session.rs:424-429 函数返回 `true`，H2wSession::run 据此认为成功
- src/core/controller.rs:298-310  `status = if msg.error.is_some() { Error } else { Finished }`
- src/gui/app.rs:473  `if (snapshot.status == TaskStatus::Finished && snapshot.total_bytes_processed > 0) || self.state.audit_record.read_tested`
- src/core/report.rs:178-179  is_fake 只看 stats，不看是否跑满
- src/gui/app.rs:849-853  非假盘分支硬编码「全盘底层 Direct I/O 检验 100% 吻合」+「[AAA级健康介质]」

**上游差异(文件:行号)**
- .upstream_f3/src/f3read.c 的统计在读完所有预期数据后才汇总，缺失的文件计入 missing 并打印 "Data OK" 之外的 "Missing files" 段；不存在「读到一半停下还算 OK」的路径。

**攻击者如何利用**
1. 用户对 64GB 假盘跑校验，读到 8GB 后觉得太慢，点「停止校验」（app.rs:407-408）。
2. session.rs:506 只 break，尾部发 Completed(error=None)。
3. controller.rs:298 判 Finished；app.rs:473 因 bytes>0 渲染报告卡。
4. 假盘在第 8GB 之后丢失的扇区根本没被读过，stats 全 0，is_fake=false，界面显示 AAA 级健康介质。
5. 用户截图作为维权证据，实际上把自己送进对方手里。

**一句话修复方向**
内层循环 break 后必须检查 cancel_flag 并改发 Aborted，且 report 层增加「已校验字节数 == 预期字节数」作为 AAA 评级的前置条件。

---

### 15. 校验只覆盖存在的 .h2w 文件，从不与设备标称容量比对，部分写入即显示 AAA（Critical）

**现象**
execute_verify 的 total_expected_bytes 来自实际文件长度求和（session.rs:453-455），report 的 announced_bytes 也回落到同一个值（report.rs:174）。因此只要写阶段只写了 1GB 就断了，校验阶段只校验那 1GB，两者相等，虚标比例恒为 0。

**证据(文件:行号)**
- src/core/session.rs:453-455
```rust
let total_expected_bytes: u64 = files.iter()
    .map(|(_, p)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(GIGABYTE))
    .sum();
```
- src/core/report.rs:174  `let announced_bytes = device.map(|d| d.total_bytes).unwrap_or(telemetry.total_bytes_expected);`
- src/gui/app.rs:488-489  `bytes_processed` 与 `total_bytes_expected` 都由 `snapshot.total_bytes_processed` 反填，构造出的遥测天然「已处理 == 预期」
- src/gui/app.rs:504-511  build_verify_audit 传入的 dev 来自 app.rs:474 `inventory.active_target()`，而 active_target 在 selected_device 被清空时返回 None（devices.rs:718-720），此时 announced 直接退化为文件长度

**上游差异(文件:行号)**
- .upstream_f3/src/f3read.c 依据 `--start-at` 之外的全部预期分卷统计 `missing`，文件缺失会明确报告，不会把「只存在 1GB 文件」当成「整盘 1GB 全好」。

**攻击者如何利用**
假盘固件不需要做任何事，只要让主机在上层写入阶段失败（或用户手工设了「容量上限」app.rs:965-1000，或写到一半磁盘满），校验阶段就只对着已写部分自证清白。

**一句话修复方向**
校验阶段必须取物理设备 total_bytes 作为 expected，并在总校验量显著小于标称容量时直接输出「未覆盖全盘，结论无效」。

---

### 16. 校验把缓冲末尾不满 512 字节的残余静默丢弃（Medium）

**现象**
读循环拿到 bytes_read 后先向下对齐到 512 再校验，落下的尾字节既不校验也不报错。

**证据(文件:行号)**
- src/core/session.rs:523-524
```rust
let aligned = bytes_read - (bytes_read % SECTOR_SIZE);
if aligned == 0 { break; }
```
- src/core/session.rs:526  `validate_sectors(&buf[..aligned], ...)` 只校验对齐部分
- src/core/direct_io.rs:198-235  read_at 返回实际字节数，NO_BUFFERING 下短读是允许的

**上游差异(文件:行号)**
- .upstream_f3/src/libfile.c 以块为单位读写并在最后一块按实际长度处理，不存在「尾部字节直接丢弃」的路径

**攻击者如何利用**
固件只要让每个读请求少返回几个字节（例如 4095），尾部字节就永久跳过校验。需要固件配合，且现有实现里 filenames 长度总是 1GiB 整数倍，实际影响有限，故定为 Medium。

**一句话修复方向**
把非对齐尾部单独拷进对齐缓冲补齐后校验，或直接判定为不完整读取并报错。

---

### 17. 卷选择器完全没有安全过滤，C 盘可以一键被选为写入目标（Critical，独立于第 6 条的机制）

**现象**
物理磁盘下拉框有 is_safe 门禁（device_selector.rs:34、57），逻辑卷下拉框没有任何过滤，列出所有盘符并直接接受点击。写入页第一行就是这个下拉框（app.rs:256）。

**证据(文件:行号)**
- src/gui/components/device_selector.rs:88-99
```rust
let volumes = inventory.volumes().to_vec();
for (idx, vol) in volumes.iter().enumerate() {
    let resp = ui.selectable_label(selected_idx == idx, vol.display_name(),);
    if resp.clicked() { inventory.select_volume(idx); }
}
```
- src/core/devices.rs:541-606  enumerate_volumes 只按 GetDriveTypeW 过滤 0/1，系统卷、内部盘、网络盘、光驱全部保留
- src/gui/app.rs:1163  `let target_dir = self.state.inventory.selected_path();`
- src/gui/app.rs:261-266  「开始写入」只有 `snapshot.status != Running` 一个条件，没有任何卷校验
- src/core/session.rs:314-364  逐 1GiB 写满直至 ERROR_DISK_FULL

**上游差异(文件:行号)**
- 上游 f3write.c 直接接受用户命令行给出的目录，不存在自动枚举与自动选优；本项目 devices.rs:733-745 的 auto_bind_initial_selection 会在没有可移动盘时把 `selected_volume_index` 设为 0（即 A: 之后的第一个盘），把「自动帮你选好」这件事本身变成了风险面

**攻击者如何利用**
用户误操作即可：插入假盘前先打开软件（此时 auto_bind 选中的是第一个非可移动卷，通常是 C:），再插盘，界面顶部「选择磁盘」显示的是 C:，用户看都不看直接点「开始写入」。以 Direct I/O 灌满 C 盘，系统随即蓝屏或无法启动。

**一句话修复方向**
卷选择器必须按 volume 反查到的物理盘 is_safe_target 过滤，且 select_volume 也要同步强制校验，未通过时下拉框只显示当前卷并置灰。

---

### 18. 超级电容 / 写缓存：flush 只用 FlushFileBuffers，没有向主控下发 SCSI SYNCHRONIZE CACHE / NVMe Flush 的独立证明（Medium，推测级）

**现象**
flush() 仅在 is_write 时调 FlushFileBuffers（direct_io.rs:152-163）。在同步、无重叠句柄上，Windows 会等待 IRP_MJ_FLUSH_BUFFERS 完成，语义上确实会下发到设备，但它不区分「设备声明已落盘」与「设备缓存在掉电前不落盘」。更关键的是，探测路径上的探针与冲刷写入没有独立的缓存排空证据：探针写完立刻回读，而回读命中主控 DRAM 就等于通过。

**证据(文件:行号)**
- src/core/direct_io.rs:152-163  flush 只有 FlushFileBuffers，失败返回 Err
- src/core/probe.rs:221-228  write_at 成功后立刻 `dev.flush()`，随后 scratch_window 冲刷、再回读本扇区
- src/core/direct_io.rs:99-101  open_raw_device 只用 FILE_FLAG_WRITE_THROUGH + NO_BUFFERING，未使用 FILE_FLAG_NO_COMPRESSION、未设置 IOCTL_STORAGE_SET_PROPERTY 的写缓存策略
- 本项目没有任何地方下发 IOCTL_DISK_SET_CACHE_INFORMATION 或向 USB BOT 发 SCSI SYNCHRONIZE CACHE（Cargo.toml:16-27 也没有引入对应 feature）

**上游差异(文件:行号)**
- .upstream_f3/src/libdevs.c:451-470  bdev_write_blocks 在写后 `fsync` 并 `posix_fadvise(POSIX_FADV_DONTNEED)` 丢弃内核页缓存，让下一次读必须回设备
- .upstream_f3/src/libdevs.c:788-816  bdev_usb_reset 在需要时用 USBDEVFS_RESET 真正复位 USB 设备，这是本项目完全没有的能力

**攻击者如何利用**
推测级：带 DRAM 缓存的假盘主控只要保证「刚写的 512 字节探针能被后续读命中」，探测的自我回读就永远通过；cache_flush 冲刷写入是写别的扇区，只要这些写也被吸收进缓存而不强制落盘，冲刷就无效。真正能防住的只有冲刷量超过主控缓存容量这一条，而本项目 CACHE_FLUSH_MAX_SECTORS 的封顶值由 probe.rs 常量决定，不随设备缓存大小自适应。

**一句话修复方向**
探测结束前对整盘（或大比例）调用一次 flush 后重新读回基准探针，并把冲刷深度按「标称容量的一定比例」而非固定上限来定。

---

### 19. 停止探测后 status 被置为 Finished、耗时与 physiology 全部清零，日志与维权档案里看不到「被中止」（Medium）

**现象**
controller.rs 的 probe 分支在收到带 error 的终止遥测时，只要 cancel_flag 为真就判 Finished，不判 Error/Aborted。

**证据(文件:行号)**
- src/core/controller.rs:334-342
```rust
let is_cancelled = self.cancel_flag.load(Ordering::Acquire)
    || msg.error.as_deref().map(|e| e.contains("用户中止") || e.contains("手动中止")).unwrap_or(false);
if is_cancelled { self.snapshot.status = TaskStatus::Finished; ... }
```
- src/core/controller.rs:326-331  `self.snapshot.progress_pct = msg.progress_pct;` 与 `status_message = msg.current_phase.clone()` 每帧覆盖，中止后只剩最后一句文案
- src/core/controller.rs:444-474  reset_for_new_task 清空 total_bytes_processed / duration / physiology
- src/gui/app.rs:128-152  完成边界只按 kind 写 audit_record，Probe 不属于 Write/Verify，不写任何记录
- src/gui/app.rs:538-545  中止走黄字分支，文案来自 probe.rs:373 的「用户中止了探测任务」

**上游差异(文件:行号)**
- .upstream_f3/src/f3probe.c:561  `return fake_type == FKTY_GOOD ? 0 : 100 + fake_type;` 退出码携带结论，进程中止时 shell 拿到非零码，调用方可区分

**攻击者如何利用**
不是安全漏洞而是取证缺陷：用户中止后界面顶部状态是绿色「已完成」，导出的 Markdown 报告里没有中止字段，无法证明探测跑到哪一步就停了。

**一句话修复方向**
probe 分支的 is_cancelled 应当映射到独立的 TaskStatus::Aborted 而不是 Finished，并在 audit_record 中记录中止发生的扇区与已执行探针数。

---

### 20. 托盘消息循环的 Destroy 分支只发 WM_TRAY_CMD 而不发 PostQuitMessage，destroy 的 join 可能永久阻塞主线程（High，推测级需实测）

**现象**
tray_wnd_proc 处理 WM_TRAY_CMD 时先调 DestroyWindow(hwnd)，再 return LRESULT(0)。DestroyWindow 会对同一个窗口同步派发 WM_DESTROY，而 WM_DESTROY 分支里调用了 PostQuitMessage，消息循环应当能退出。但 TrayManager::destroy 在 cmd_tx.send 之后又 PostMessageW 到同一个 hwnd；若 DestroyWindow 已经先执行完，这次 PostMessage 投递的对象已销毁，消息丢失。此路径是否必然卡死取决于 DestroyWindow 内部的同步派发是否一定先于这次 PostMessage 到达，属于时序相关的推测级风险。

**证据(文件:行号)**
- src/gui/tray.rs:131-159  WM_TRAY_CMD 分支：should_destroy → remove_tray_icon + DestroyWindow；WM_DESTROY 分支才 PostQuitMessage
- src/gui/tray.rs:371-378  `while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() { ... }` 只有收到 WM_QUIT 才退出
- src/gui/tray.rs:417-434  destroy() 里 send(Destroy) 后无条件再 PostMessageW，然后 `handle.join()`
- src/gui/app.rs:105-109  未开启「最小化到托盘」时关闭窗口走 `t.destroy()`，阻塞在 UI 线程
- src/gui/app.rs:1149-1156  「强制中止并退出」同样先 `tray.destroy()` 再 Close

**上游差异(文件:行号)**
- 上游无 GUI、无托盘线程，无对应语义

**攻击者如何利用**
环境异常即可触发：用户在没有任何任务运行时关闭窗口，主线程卡在 join 上，窗口白屏不响应，只能任务管理器结束进程。

**一句话修复方向**
destroy 里直接 PostThreadMessageW(thread_id, WM_QUIT)，或在 Destroy 命令后不再重复 PostMessage，并对 join 加超时。

---

### 21. 上报到桌面的脚本与 Markdown 报告都用 %USERPROFILE%\Desktop 硬拼，重定向的桌面（OneDrive / 企业重定向）会导致静默写错位置或失败（Low）

**现象**
导出 diskpart 脚本用 `%USERPROFILE%\Desktop\f3_fix.diskpart`，证据报告与审计报告用 USERPROFILE 拼 Desktop 且回退到 `"."`。启用 OneDrive 文件夹备份或域重定向后，真实桌面在 `%USERPROFILE%\OneDrive\桌面` 或网络路径。

**证据(文件:行号)**
- src/gui/app.rs:620-625  固定文件名 f3_fix.diskpart，覆盖写
- src/core/report.rs:680-700  export_to_desktop 用同一个 get_desktop_dir
- src/core/report.rs:722-736  三级回退，最后落到当前工作目录 `PathBuf::from(".")`
- src/core/paths.rs:28-48  AppPathInfo 里另有 evidence_dir / temp_dir，但没有 SHGetKnownFolderPath 调用

**上游差异(文件:行号)**
- .upstream_f3/src/f3probe.c:507-517  只是把 f3fix 命令打印到标准输出，由用户自己决定存哪

**攻击者如何利用**
用户误操作：启用 OneDrive 的用户在真正桌面上找不到脚本，以为导出失败；回退到 `"."` 时文件落在当前工作目录（可能是 System32），且固定文件名会静默覆盖同名的旧脚本。

**一句话修复方向**
用 SHGetKnownFolderPath(FOLDERID_Desktop) 取真实桌面，导出文件名带时间戳且用 CREATE_NEW。

---

### 22. 单元测试把 MemoryBlockDevice 当成 F3Probe 的唯一被测载具，四条真实设备语义没有被模拟（High）

**现象**
probe 的全部单测跑在 MemoryBlockDevice 上。该仿真设备没有短写、没有 EIO、没有 4Kn 扇区、flush 恒成功，而真实路径上这四个语义每一个都能改变判定结果。

**证据(文件:行号)**
- src/core/direct_io.rs:374-393  write_at 越界返回 Err、否则 Ok(len)（没有短写）；flush `Ok(())` 恒成功；sector_size 用 trait 默认 512
- src/core/direct_io.rs:395-434  仅 2 个单测：读写回环、假回绕
- src/core/probe.rs:221-228  生产路径上 `dev.write_at` 与 `dev.flush` 任一失败即判 ProbeTrap::BitBucket —— 这个分支在仿真设备上永远不会被执行
- src/core/controller.rs:199  `let total_sectors = target.device.total_bytes / 512;` 用 512 假设推算扇区数；src/core/direct_io.rs:105 打开物理设备时同样把 sector_size 硬编码为 512
- .github/workflows/ci.yml  runner 是 windows-latest、非管理员身份，所有 `CreateFileW(\\.\PhysicalDriveN)` 路径在 CI 中必然失败，因此这些代码从未被实际执行过

**上游差异(文件:行号)**
- .upstream_f3/src/libdevs.c:941-957  create_block_device 在真实设备上取 BLKSSZGET 得到物理扇区大小，再据此设置 block_order；本项目把 512 写死，4Kn 设备上的判断没有被任何测试覆盖

**攻击者如何利用**
这不是运行时可利用的漏洞，而是门禁失效：真盘被误判成 BitBucket（短写或瞬时 EIO）、4Kn 盘上 offset 与长度不被接受这类问题，全部只能在真机上暴露，而 CI 永远全绿。

**一句话修复方向**
让 MemoryBlockDevice 支持注入短写、EIO、flush 失败与 4Kn sector_size，并为 probe 增加四个对应的失败注入单测。

---

## 汇总表

| 序号 | 严重度 | 主题 | 主证据位置 |
| --- | --- | --- | --- |
| 1 | Critical | RemovableMedia 单独放行，NVMe 系统盘可直达物理写 | src/core/devices.rs:233 |
| 2 | Critical | 卷锁守卫与写句柄不互斥，空守卫照样写裸扇区 | src/core/devices.rs:386 / controller.rs:207 |
| 3 | Critical | clean 范围过大且磁盘号在生成脚本时冻结 | src/core/fix.rs:182-199 |
| 4 | High | 临时 diskpart 脚本名可预测、非独占创建 | src/core/fix.rs:47-58 |
| 5 | High | 探针写入无备份，取消后原数据永久丢失 | src/core/probe.rs:305-323 |
| 6 | Critical | 卷选择器无安全过滤 | src/gui/components/device_selector.rs:88-99 |
| 7 | Medium | 自动清理不看取消标志 | src/core/session.rs:613 |
| 8 | Medium | diskpart 成败判定恒假成功 | src/core/fix.rs:102-108 |
| 9 | High | clippy 无 -D warnings | .github/workflows/ci.yml:26-27 |
| 10 | High | cargo test 未覆盖全部目标与架构 | .github/workflows/ci.yml:29-30 |
| 14 | Critical | 校验中途取消判 Finished 且显示 AAA | src/core/session.rs:505-510 |
| 15 | Critical | 校验量只等于已写文件长度，从不比标称容量 | src/core/session.rs:453-455 |
| 16 | Medium | 尾部不满 512 字节被丢弃 | src/core/session.rs:523-524 |
| 17 | Critical | 卷下拉框可直选 C 盘开始写入 | src/gui/app.rs:256 / 1163 |
| 18 | Medium | flush 无向主控下发 SYNCHRONIZE CACHE 的证明 | src/core/direct_io.rs:152-163 |
| 19 | Medium | 停止探测被判 Finished，取证链缺失 | src/core/controller.rs:334-342 |
| 20 | High | 托盘 destroy 的 join 可能永久阻塞主线程 | src/gui/tray.rs:417-434 |
| 21 | Low | 桌面路径硬拼，OneDrive 重定向下失效 | src/core/report.rs:722-736 |
| 22 | High | 仿真设备掩盖 probe 的真实失败语义 | src/core/direct_io.rs:374-393 |

安全基线说明：本次审计全部为静态阅读，未执行任何编译命令，未修改任何源码文件；报告文件 docs/audit/03-platform.md 为本轮唯一写入目标。

