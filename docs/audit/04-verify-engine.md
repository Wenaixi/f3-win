# 核实报告：核心引擎组（session / direct_io / controller）

说明：本组条目由 Lead 亲自逐行核实。四个核实子代理在本会话中均未能真正执行（返回 subagentId 但无产出，目录中不存在 04 到 07 号文件），因此所有结论以 Lead 直接阅读源码为准。

## V-E3 成立（Critical）最后一个分卷不 flush 且报完成

声称：最后一个分卷写满时不 flush 且报成功。

证据：src/core/session.rs:314 的主循环 `while file_offset < GIGABYTE` 共有四条退出通路，逐条核实如下。

通路一，写满 1GB 分卷：src/core/session.rs:391-395

```rust
if file_offset >= GIGABYTE {
    if let Some(f) = direct_file.as_mut() {
        let _ = f.flush();
    }
}
```

flush 被调用，但返回值用 let _ = 丢弃。

通路二，磁盘已满：src/core/session.rs:357-367

```rust
if is_disk_full {
    file_full = true;
    if file_offset > 0 {
        let _ = writer.truncate_to(file_offset);
        let _ = writer.flush();
    } else {
        drop(direct_file.take());
        let _ = std::fs::remove_file(&file_path);
    }
    break;
}
```

flush 被调用，返回值同样丢弃。file_offset 为 0 时直接删文件不 flush。

通路三，用户取消：src/core/session.rs:315-317

```rust
if self.cancel_flag.load(Ordering::Relaxed) {
    break;
}
```

直接 break，不进入 line 391 的判断（file_offset < GIGABYTE），因此这条路径上完全没有 flush。

通路四，物理故障：src/core/session.rs:384 `return false;`，同样不 flush。

最终落点：src/core/session.rs:404-418

```rust
let is_terminal = self.config.mode == H2wMode::Write;
let _ = tx.send(self.make_telemetry_with_tracker(
    if is_terminal { H2wPhase::Completed } else { H2wPhase::Writing },
    ...
    is_terminal,
    None,
    &tracker,
));
```

判断：成立，且比声称更严重。取消路径与物理故障路径上完全没有 flush 调用；写满与磁盘已满路径有 flush 但返回值被丢弃，失败静默。四条通路最终都发送 Completed 且 is_terminal 为 true（写模式）。

对照上游：.upstream_f3/src/f3write.c:233 每个 chunk 写完立即 fdatasync 并检查返回，:234-237 保留首个错误。上游是逐 chunk 同步落盘，本项目是整卷结束才可能 flush 一次，且不检查。

最小修复点：
1. src/core/direct_io.rs:166-195 write_at 返回字节数必须等于请求长度，否则返回 Err。
2. src/core/session.rs:361 与 :393 的 let _ = 必须改为错误传播，flush 失败要上报 Failed。
3. src/core/session.rs:315-317 的取消路径与 :384 的故障路径在退出前也必须 flush。
4. src/core/session.rs:404-418 只有在确实 flush 成功后才允许发 Completed。

## V-E4 成立（High）WriteFile 返回 0 字节会死循环

证据：src/core/session.rs:327-331

```rust
match writer.write_at(&buf[..to_write_aligned], file_offset) {
    Ok(written) => {
        let actual = std::cmp::min(written, to_write);
        total_written += actual as u64;
        file_offset += actual as u64;
```

src/core/direct_io.rs:187-188

```rust
match res {
    Ok(_) => Ok(bytes_written as usize),
```

判断：成立。WriteFile 成功但 bytes_written 为 0 时，actual 为 0，file_offset 不推进，src/core/session.rs:314 的 while 条件 `file_offset < GIGABYTE` 永远为真，且 cancel_flag 未被置位时无其他出路，进程挂死在写入循环里。上游 f3write.c:163-170 对不足量的写入直接 assert 失败。

最小修复点：在 src/core/direct_io.rs 的 write_at 内校验 bytes_written 等于 buf.len()，不等即返回 Err，从根源消除 0 字节与短写两种情况。

## V-E5 成立（High）短写静默忽略

证据：src/core/direct_io.rs:187-188 直接返回 bytes_written，未与 buf.len() 比较。
src/core/session.rs:329 用 std::cmp::min(written, to_write) 把“少写了”当成正常进度推进。

判断：成立。短写时 total_written 与 file_offset 都只推进实际写入量，剩余部分由下一轮循环重新生成并写入。因为 fill_sectors 是确定性的，重写内容一致，所以短写本身不产生数据错误；但若驱动返回 0 则演变为 V-E4 的死循环。真正的危害是掩盖了设备异常。

## V-E6 成立（Critical）4Kn 支持是空壳

证据：src/core/direct_io.rs:147-149 存在 `pub fn sector_size(&self) -> usize { self.sector_size }` 读取接口，但全仓对 set_sector_size 的调用点为零。
src/core/direct_io.rs:167-172 与 :199-204 的 write_at 与 read_at 断言只用 self.sector_size 校验，而该字段在构造时恒为 512。

判断：成立。物理扇区为 4096 的设备上，512 字节对齐的缓冲区配合 FILE_FLAG_NO_BUFFERING 会被内核拒绝，返回 ERROR_INVALID_PARAMETER (87)。src/core/probe.rs:222-224 把该失败归约为 ProbeTrap::BitBucket，正品 4Kn 盘被判为位黑洞假盘，且 error 字段为 None。

## V-E7 成立（High）仿真设备过于宽容

证据：src/core/direct_io.rs:374-393 的 MemoryBlockDevice。

判断：成立。该仿真设备不支持短写、不产生 EIO、flush 恒返回 Ok、扇区恒为 512。因此 src/core/probe.rs 的 BitBucket 失败分支与 4Kn 分支在 CI 中永不执行，V-E4 的零字节死循环也无法被现有测试发现。

## V-E8 成立（Medium）flush 只有 FlushFileBuffers

证据：src/core/direct_io.rs:151-163

```rust
pub fn flush(&mut self) -> Result<(), String> {
    if self.is_write {
        let res = unsafe { FlushFileBuffers(self.handle) };
```

全工程 grep SCSI、SYNCHRONIZE、NVMe、IOCTL_STORAGE 均无刷盘相关调用。

判断：成立。注释声称向主控下发 SCSI SYNCHRONIZE CACHE / NVMe Flush，实际只有 FlushFileBuffers 一个调用。上游 libdevs.c:451-470 是 fsync 配合 posix_fadvise(DONTNEED)，libdevs.c:788-816 另有 USB 复位。

## 本组总评

V-E3 与 V-E4 的共同根因是同一个：write_at 与 flush 的失败信号在调用链上被系统性吞掉。修 write_at 的返回值校验（一处）可以同时消灭短写、零字节死循环、以及部分 flush 语义问题；修 session 的 flush 传播（三处）可以消灭最后一卷丢数据。这两处是最高优先级的单点根因。
