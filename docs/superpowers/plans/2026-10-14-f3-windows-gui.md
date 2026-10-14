# F3-Win 极致性能 Windows GUI 移植实施计划

## 1. 项目概述与设计原则
本计划旨在将 Linux 开源扩容盘检测工具 F3 (Fight Flash Fraud) 移植至 Windows 平台，打造一个单文件绿色原生可执行程序（Rust + Win32 API + egui）。

### 核心铁律
1. 严禁本地编译：本地工作区绝对不调用 cargo build、rustc、gcc 等任何编译器；所有编译检查、测试与跨平台产物发布一律委托 GitHub Actions CI 执行。
2. 完美性能与忠实还原：
   - 算法 100% 忠实还原原版 F3：线性同余 LCG（next = r * 4294967311 + 17）、首字 offset、7-bit popcount 容错校验；
   - Direct I/O：FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH 彻底阻断系统文件缓存伪速；
   - 内存与管线：VirtualAlloc 4KB 扇区对齐双缓冲 + Overlapped I/O，LCG 随机数计算与硬件物理刷盘并发重叠；
   - SIMD 加速：AVX2 / SSE4.2 / NEON 向量化加速数据生成与比对，饱和 1GB/s~3GB/s 读写总线。
3. 安全第一：底层物理探测强制过滤硬件总线（IOCTL_STORAGE_QUERY_PROPERTY），硬隔离系统盘与固定磁盘，破坏性操作自动卸载挂载点并强提醒。

---

## 2. 模块划分与代码目录树

```text
f3-win/
├── .github/
│   └── workflows/
│       └── ci.yml                 # CI 流水线：x86_64 与 aarch64 双架构矩阵、LTO 构建、Release 自动化发布
├── Cargo.toml                     # 项目依赖清单与极值优化 Profile (LTO, strip, opt-level="z")
├── CLAUDE.md                      # 项目核心规范与决策记忆库
├── docs/
│   └── superpowers/
│       └── plans/
│           └── 2026-10-14-f3-windows-gui.md
└── src/
    ├── main.rs                    # 程序入口：WinMain 初始化、DPI 感知、启动 egui 主窗口
    ├── core/
    │   ├── mod.rs
    │   ├── algo.rs                # 算法核心：LCG 生成器、SIMD 向量化优化、状态枚举 (Good, Changed, Overwritten, Bad)
    │   ├── aligned_buf.rs         # 内存管理：基于 VirtualAlloc 的 4KB 扇区对齐安全缓冲区
    │   ├── direct_io.rs           # Win32 Direct I/O 驱动层：CreateFileW 无缓冲重叠 I/O 包装器
    │   ├── writer.rs              # f3write 引擎：双缓冲生产者-消费者管线，生成 1.h2w 文件
    │   ├── reader.rs              # f3read 引擎：高速流式校验器，坏块与覆盖分析
    │   ├── probe.rs               # f3probe 引擎：跳跃扇区算法，快速判定真实物理容量与回绕边界
    │   ├── fix.rs                 # f3fix 引擎：合规分区重算、Win32 磁盘布局写入与 diskpart 脚本导出
    │   └── devices.rs             # 磁盘与安全层：总线类型枚举、挂载卷锁定/卸载 (FSCTL_LOCK/DISMOUNT)
    └── gui/
        ├── mod.rs
        ├── app.rs                 # egui 主状态机与 Tab 切换（文件测试、快速探测、分区修复、设置）
        ├── state.rs               # 跨线程状态与监控遥测数据传输定义
        └── components/
            ├── mod.rs
            ├── plot.rs            # 实时读写速率折线图（基于 egui_plot，动态捕捉 SLC 掉速）
            ├── heatmap.rs         # 扇区健康可视化热力块矩阵（高性能自定义 Painter）
            └── device_selector.rs # 带硬件总线徽标与系统盘红锁的安全磁盘选择下拉框
```

---

## 3. 分阶段实施任务（Bite-sized Tasks）

### 阶段一：工程脚手架与 CI 矩阵建立
- 任务 1.1：编写 `Cargo.toml`，配置依赖项（`windows-sys` / `windows`、`eframe`、`egui`、`egui_plot`、`crossbeam-channel`）及 release profile（`opt-level = "z"`, `lto = true`, `strip = true`, `panic = "abort"`）。
- 任务 1.2：编写 `.github/workflows/ci.yml`，建立 GitHub Actions 自动化编译流水线：
  - Matrix 包含 `x86_64-pc-windows-msvc` 与 `aarch64-pc-windows-msvc`；
  - 任务流：`cargo fmt --check`, `cargo clippy`, `cargo test`；
  - Tag 推送时执行编译构建并上传绿色单文件可执行文件至 Release。

### 阶段二：算法核心与 SIMD 向量化实现
- 任务 2.1：编写 `src/core/algo.rs`，严格实现原版 F3 的 LCG 公式：
  - `next_random_number(r) = r.wrapping_mul(4294967311).wrapping_add(17)`；
  - `fill_block_scalar(buf, offset, salt)`：扇区首个 64 位写入 `offset`，其余槽位用 LCG 填充；
  - `validate_block_scalar(buf, expected_offset, salt)`：比对数据，并用 `count_ones()` (硬件 popcount) 计算位翻转差值，容错阈值设定为 7，准确归类 `BlockState::Good`, `BlockState::Changed`, `BlockState::Overwritten`, `BlockState::Bad`。
- 任务 2.2：在 `src/core/algo.rs` 中引入运行时特性探测与 AVX2 / SSE4.2 SIMD 批量填充与比较实现，保证大块内存处理跑满 CPU 单核内存吞吐（>10GB/s），彻底消除 1GB/s 闪存读写时的 CPU 瓶颈。
- 任务 2.3：编写单元测试用例，比对原版 F3 C 代码生成的确定性魔数向量，验证测试数据的一致性。

### 阶段三：Windows Direct I/O 与对齐内存管理
- 任务 3.1：编写 `src/core/aligned_buf.rs`：
  - 封装 `VirtualAlloc` (`MEM_COMMIT | MEM_RESERVE`, `PAGE_READWRITE`) 与 `VirtualFree`；
  - 强制 4096 字节（甚至 2MB 大页）物理内存页对齐；
  - 实现安全 RAII 释放与分片切片借用方法。
- 任务 3.2：编写 `src/core/direct_io.rs`：
  - 封装 Win32 `CreateFileW`，传入 `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`；
  - 支持异步重叠 I/O（Overlapped I/O）与事件等待句柄，实现非阻塞硬件写入与读取。
  - 处理文件尾部不足 4KB 对齐时的 padding 策略与真实长度截断处理。

### 阶段四：文件级全盘测试引擎（f3write & f3read）
- 任务 4.1：编写 `src/core/writer.rs`：
  - 实现双缓冲异步生产者-消费者管线：线程 A 负责生成 SIMD 随机数填充 Buffer，线程 B 负责提交 Direct I/O 写入；
  - 自动管理每 1GB 单文件划分（`1.h2w`, `2.h2w`...）；
  - 实时采集瞬时速度（每 500ms 采样）、累计写入量、ETA 计算，并通过通道发送至 GUI 线程。
- 任务 4.2：编写 `src/core/reader.rs`：
  - 实现 Direct I/O 流式读取已有 `.h2w` 文件；
  - 调用 SIMD 比对引擎统计坏块、覆盖块、变异块与健康块；
  - 支持读取过程中的瞬时读速采样与健康色块矩阵元数据收集。

### 阶段五：硬件总线隔离与裸盘极速探测（f3probe）
- 任务 5.1：编写 `src/core/devices.rs`：
  - 枚举系统物理磁盘 `\\.\PhysicalDriveX` 与逻辑卷（盘符）；
  - 发送 `IOCTL_STORAGE_QUERY_PROPERTY` 查询 `STORAGE_DEVICE_DESCRIPTOR`，提取 `BusType`（如 `BusTypeUsb`）；
  - 默认硬隔离：非 USB/SD/MMC 类型的设备强制锁定为禁用状态；
  - 提供卷挂载点反查与卸载功能：调用 `FSCTL_LOCK_VOLUME` 和 `FSCTL_DISMOUNT_VOLUME` 独占物理驱动器。
- 任务 5.2：编写 `src/core/probe.rs`：
  - 忠实实现 F3 的跳跃式探测算法：针对 2^N 扇区进行特征标记写入与快速读回；
  - 判定控制器缓存大小（Cache Overwhelm）与真实扇区回绕边界（Wrap-around）；
  - 输出探测报告：标称容量（Announced Size）、实测物理容量（Real Size）、回绕跨度（Wrap Border）。

### 阶段六：分区自愈修复引擎（f3fix）
- 任务 6.1：编写 `src/core/fix.rs`：
  - 轨道 1（原生自愈）：利用 Win32 API 写入全新 MBR 分区表，限制分区首末扇区在探测到的有效区域内，并调用格式化 API 初始化为 exFAT / FAT32；
  - 轨道 2（透明脚本）：生成合规的 Windows `diskpart` 批处理命令文件，提供一键预览与导出保存。

### 阶段七：轻量原生 GUI 仪表盘与可视化构建
- 任务 7.1：编写 `src/gui/app.rs` 与 `src/gui/state.rs`：
  - 设置 egui 极简现代深色主题，支持高 DPI 屏幕自动缩放；
  - 定义无锁线程安全的消息通信通道，将核心引擎状态同步到界面渲染帧。
- 任务 7.2：编写 `src/gui/components/plot.rs`：
  - 使用 `egui_plot` 绘制实时平滑读写速度折线图，标识 SLC Cache 拐点。
- 任务 7.3：编写 `src/gui/components/heatmap.rs`：
  - 高性能色块热力矩阵：将全盘映射为网格，根据扇区状态（Good 绿色, Bad 红色, Changed 黄色, Overwritten 紫色）直接用 Painter 批量绘制矩形，毫秒级帧率，零卡顿。
- 任务 7.4：编写 `src/gui/components/device_selector.rs`：
  - 下拉磁盘选择组件，突出显示 USB 标识，对内置系统盘显式呈现锁头与禁用态。
- 任务 7.5：编写测试报告弹窗与一键复制功能。

### 阶段八：CI 自动化构建、验证与发布
- 任务 8.1：完善代码结构，执行静态代码自检与格式规范审查；
- 任务 8.2：提交代码并触发 GitHub Actions CI 矩阵构建，验证 x86_64 与 aarch64 双目标编译通过；
- 任务 8.3：在 CI 产物中验证单文件体积（<10MB）与运行能力。
