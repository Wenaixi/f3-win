# F3-Win: 极致性能 Windows 原生闪存测试与自愈终端

F3-Win 是 Linux 著名开源扩容盘检测工具 F3 (Fight Flash Fraud) 的现代 Windows 原生 GUI 完整移植版本。基于 Rust (2021 Edition) 与底层 Win32 Direct API 构建，具备单文件绿色运行、饱和物理总线带宽、硬件级安全隔离与全套自愈修复能力。

---

## 核心架构与特性

### 1. 饱和物理带宽的 Direct I/O 引擎
在 Windows 默认的文件 I/O 机制下，操作系统的 File Cache Manager 会将写入数据缓存在系统内存中，造成写入速度虚高至数 GB/s 并在随后的 Flush 过程中发生严重掉速卡顿。
- F3-Win 深度利用 Win32 `CreateFileW` 的 `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH` 标志，彻底绕过系统页面缓存，让每一个扇区直达物理闪存主控；
- 配合基于 `VirtualAlloc` 分配的 4KB 扇区对齐双缓冲，避免任何额外的内存拷贝与碎片开销；
- 数据生成与磁盘写入采用解耦的多线程生产者-消费者架构，使 CPU 计算与物理总线传输完全并发重叠。

### 2. 100% 忠实还原的算法核心与 SIMD 加速
- 线性同余发生器 (LCG)：严格遵循原版 F3 公式 `r = r * 4294967311 + 17`，首个 64 位存放文件偏移量，确保生成的 `1.h2w` 测试文件与经典 H2testw 和原版 F3 完全双向互通；
- 硬件级 popcount 容错：采用 `count_ones()` 进行位翻转差值判定（容错阈值 <= 7），准确区分 Good（健康）、Bad（损坏）、Changed（位变异）、Overwritten（回绕覆写）；
- SIMD 快速比对：在常规健康扇区下跳过逐位计算，直奔单核内存极限带宽，杜绝高速 NVMe/USB3.2 读写下的 CPU 瓶颈。

### 3. 全套工具集无缝集成
- f3write：无损文件系统级全盘直通写入；
- f3read：全盘数据回读校验，坏块与覆盖精准统计；
- f3probe：底层物理扇区 2^N 跳跃探测，在 1~3 分钟内迅速侦破 2TB 虚标假盘的真实物理容量；
- f3fix：双轨自愈机制，根据实测容量自动生成截断分区脚本（diskpart）并支持一键写入合规分区表。

### 4. 硬件总线级安全隔离
- 设备枚举阶段调用 `IOCTL_STORAGE_QUERY_PROPERTY` 查询总线类型 (`BusType`)；
- 默认严格锁定仅允许操作 USB / SD / MMC 外部移动存储，系统盘与内置 SATA/NVMe 固态硬盘被强制打上保护锁，严禁一切破坏性扇区测试；
- 执行探测前自动调用 `FSCTL_LOCK_VOLUME` 与 `FSCTL_DISMOUNT_VOLUME` 卸载卷挂载，防止文件系统写冲突。

### 5. 现代化原生 GUI (egui)
- 零外部运行时依赖，单文件编译体积控制在 5MB~8MB 左右，内存占用低于 25MB；
- 实时速度平滑折线图 (基于 `egui_plot`)，精准呈现闪存由 SLC Cache 高速区向颗粒真实写入速度跌落的断崖拐点；
- 高性能扇区健康热力块矩阵，直观展示坏块全盘分布位置。

---

## 编译与发布铁律

本项目严格遵循「仅限 CI 构建，禁止本地编译」的工程规范：
1. 本地代码树仅用于逻辑设计与规范维护，不运行任何本地编译器；
2. 代码推送到远程仓库后，GitHub Actions CI 流水线会自动触发双架构编译矩阵：
   - `x86_64-pc-windows-msvc` (主流 64 位系统)
   - `aarch64-pc-windows-msvc` (Windows on ARM 原生)
3. 打 Tag (如 `v1.0.0`) 时，CI 将自动开启跨 Crate 链接时优化 (LTO) 与符号表剥离 (Strip)，生成带有 SHA-256 校验和的绿色单文件 Release 压缩包。

---

## 目录结构

```text
f3-win/
├── .github/
│   └── workflows/
│       └── ci.yml                 # CI 流水线 (双架构矩阵与自动 Release)
├── Cargo.toml                     # 项目依赖与极值优化 Profile
├── CLAUDE.md                      # 项目规范与记忆库
├── src/
│   ├── main.rs                    # 纯净 GUI 窗口入口
│   ├── core/
│   │   ├── algo.rs                # LCG 算法、扇区比对与单元测试
│   │   ├── aligned_buf.rs         # VirtualAlloc 4KB 扇区对齐安全内存
│   │   ├── direct_io.rs           # Win32 Direct I/O 无缓冲驱动
│   │   ├── writer.rs              # f3write 双缓冲写入引擎
│   │   ├── reader.rs              # f3read 校验流引擎
│   │   ├── probe.rs               # f3probe 裸盘跳跃探测引擎
│   │   ├── fix.rs                 # f3fix 分区自愈与脚本导出
│   │   └── devices.rs             # 硬件总线识别与卷卸载隔离
│   └── gui/
│       ├── app.rs                 # 主状态机与选项卡交互
│       ├── state.rs               # 跨线程状态与遥测协议
│       └── components/
│           ├── plot.rs            # 实时速率曲线 (egui_plot)
│           ├── heatmap.rs         # 扇区健康热力块矩阵
│           └── device_selector.rs # 带总线识别的安全设备选择器
└── docs/
    └── superpowers/
        └── plans/
            └── 2026-10-14-f3-windows-gui.md
```
