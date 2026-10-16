# F3-Win 领域模型字典 (Domain Model Glossary)

本文档定义 F3-Win 代码库的核心领域概念与模块职责边界。在新增代码、重构接口与编写注释时，必须严格使用以下标准词汇，禁止随意臆造别名。

## 核心领域概念

### 1. 存储资产与目标 (Storage Assets & Targets)
- **VolumeInfo (逻辑卷)**: Windows 系统中挂载的驱动器卷（例如 C:, G:），具有文件系统、可用容量与可移动属性。
- **PhysicalDeviceInfo (物理磁盘)**: 操作系统底层的存储硬件（例如 \\.\PhysicalDrive2），具有厂商型号、真实物理总线类型（USB/SD/NVMe 等）与只读/安全白名单状态。
- **StorageInventory (存储资产清单)**: 核心深层模块，内聚全系统卷与物理磁盘的枚举、动态关联（逻辑卷到底层 PhysicalDrive 的双向解析）与移动介质安全白名单过滤。对外提供统一的查询与选中状态管理。
- **StorageTarget (存储测试目标)**: 绑定了具体物理磁盘与所包含逻辑卷的高层操作门面，提供 `open_block_io()` 原子化执行 FSCTL 卷独占卸载、Win32 物理磁盘句柄获取，并将底层硬件无缝包装为 BlockIo 接缝与 RAII VolumeLockGuard 守卫。彻底消除了上层对 Win32 原始句柄与路径的直接依赖。
- **VolumeLockGuard (卷独占守卫)**: RAII 守卫结构体，持有被卸载卷的内核句柄，在析构时自动关闭句柄并由操作系统平稳恢复卷挂载。

### 2. 测试引擎与控制器 (Engines & Controllers)
- **H2wSession (H2W 测试会话)**: 深度文件级读写校验引擎，单一模块统管 Direct I/O 无系统缓存写入、多文件轮转 (.h2w)、PRNG 数据生成与 SIMD 向量化校验。
- **F3Probe (跳跃扇区探针)**: 物理裸盘急速探测引擎，跳跃式向高位物理扇区写入探针并结合缓存排空与镜像回读，数分钟内侦测假盘真实容量。
- **EngineController (引擎控制器)**: 统领后台异步工作线程、跨线程通讯管道 (Crossbeam Channels)、原子取消标志与卷锁守卫生命周期的深层中枢。统一纳管 f3write、f3read、f3probe、f3fix 与后台 .h2w 碎片清理五大任务类型，向 UI 提供完全非阻塞的调度接口与单向只读快照 TaskSnapshot。
- **TaskSnapshot (任务快照)**: 由 EngineController 每帧生成的只读聚合状态，包含平滑速率折线、扇区热力图采样、ETA 倒计时与任务完成状态。UI 界面仅对此快照进行无状态渲染。

### 3. 分区修复与防护 (Partition Remediation & Safety)
- **F3FixPlan (修复方案)**: 根据 F3Probe 探测出的真实物理容量，自动计算出的安全分区截断尺寸（预留安全边界、对齐至 1MB）与推荐文件系统方案。
- **SafePartitionFixer (安全分区自愈器)**: 具备现场硬件零时差复核 (Zero-TOCTOU) 的分区修复执行器，严禁操作系统主盘 (PhysicalDrive0) 与非移动介质。内部通过 DiskpartRunner 抽象接缝实现脚本文本生成与底层执行的完全解耦。
- **DiskpartRunner (分区脚本执行接缝)**: 最小抽象接缝 trait，提供 `run_script(&self, script: &str) -> Result<String, String>`。生产环境由 SystemDiskpartRunner 调用带有 CREATE_NO_WINDOW 的 diskpart.exe 进程；测试环境由 MockDiskpartRunner 拦截并断言脚本指令，支持脱机 CI 100% 自动化覆盖。

## 模块深浅度与接口原则
- **深层模块优先 (Deep Modules)**: 保持对外接口极简（少量方法与清晰参数），将复杂度（Win32 句柄操作、异步管道、对齐缓冲管理）完全内聚在模块内部。
- **单向数据流**: UI 呈现层禁止直接操作操作系统原生句柄与跨线程管道，所有状态流转统一遵循 `UI 事件 -> Controller 动作 -> Snapshot 快照 -> UI 渲染`。