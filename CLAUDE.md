# F3-Win 项目规范与记忆库

## 项目愿景
将开源闪存扩容盘检测利器 F3 (Fight Flash Fraud) 完美移植至 Windows 平台，打造一个单文件绿色、超高性能（饱和物理带宽）、高安全防呆的原生 Rust + Win32 Direct I/O GUI 应用程序。

## 核心架构决策全景（完全锁定）

### 1. 功能边界与工具集（全家桶）
- 文件系统级测试（f3write + f3read）：无损填充并校验 .h2w 文件，兼容任意可用分区
- 物理裸盘急速探测（f3probe）：跳跃扇区算法，数分钟内侦测出假盘真实物理容量与回绕（Wrap-around）
- 分区自愈修复（f3fix）：双轨修复机制（轨道1：Win32 API 直接重写 MBR/GPT 安全分区并格式化；轨道2：导出标准 diskpart 脚本）

### 2. 核心技术栈与编译铁律
- 核心语言：Rust (2021 Edition) + Win32 Native API (windows-rs / winapi)
- 编译铁律：严禁在本地执行任何编译（禁止运行 cargo build、cargo test、rustc、gcc 等一切本地编译器）；所有构建、静态检查与跨架构二进制发布全部由 GitHub Actions CI 自动化流水线完成

### 3. 超高性能引擎底座
- Direct I/O 架构：CreateFileW 配置 FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH，彻底绕过系统文件缓存
- 内存管理：VirtualAlloc 分配 4KB 物理扇区对齐的环形双缓冲/三缓冲，零多余内存拷贝
- 数据生成与校验 SIMD 向量化：
  - 核心算法：100% 忠实还原原版 F3 线性同余 PRNG：next = current * 4294967311 + 17，int64_array[0] = offset
  - 校验算法：利用 AVX2 / SSE4.2 / NEON 向量化指令与硬件 popcount 加速比较，在 1GB/s~3GB/s 带宽下杜绝 CPU 成为瓶颈
- 缓冲区与队列：自适应块大小（默认 1MB，支持动态升至 2MB/4MB），利用 Windows 重叠 I/O（Overlapped I/O）与多线程解耦「随机数生成」与「硬件写入」

### 4. 原生 GUI 与交互架构
- GUI 框架：egui + eframe（纯 Rust，零外部 C++ 运行时，单文件 release 产物体积约 5MB~8MB，冷启动毫秒级）
- 仪表盘与可视化：
  - 实时读写速度平滑折线图（基于 egui_plot，精准捕捉 SLC Cache 耗尽真实掉速断崖）
  - 扇区健康可视化热力块矩阵（直观渲染各扇区区间 Good/Bad/Changed/Overwritten 状态）
  - 动态精准实时 ETA 与测试摘要报告一键导出

### 5. 硬件安全防呆隔离
- 总线识别：IOCTL_STORAGE_QUERY_PROPERTY 查询 BusType，默认严格锁定仅允许 USB / SD / MMC 移动介质，自动锁死系统盘与内部 SATA/NVMe 盘
- 独占锁定：f3probe 启动前自动执行 FSCTL_LOCK_VOLUME 与 FSCTL_DISMOUNT_VOLUME 强制卸载卷挂载，防止文件系统写冲突
- 强交互确认：破坏性操作包含红字警告与二次确认

### 6. 数据兼容与防伪策略
- 双模式设计：默认 Salt = 0 与 H2testw / 原版 F3 百分百相互校验；提供可选随机 Salt 防固件作弊模式

### 7. CI 自动化流水线
- 目标平台矩阵：x86_64-pc-windows-msvc（主流 64 位）与 aarch64-pc-windows-msvc（Windows on ARM）
- 流水线任务：Push/PR 触发代码校验与测试，Tag 触发开启 LTO + Strip 的发布构建，自动发布带 SHA-256 校验和的 Release

## 文档与代码规范
- 规范：模块化、职责单一、安全第一
- 格式约束：文档与代码注释严禁使用任何 emoji
