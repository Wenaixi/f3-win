# F3-Win 全模块深度架构升级与解耦实施计划

日期: 2026-10-16
状态: 已完成 (Completed)
执行者: 猫娘 Wenxi (Lead Autonomous Mode)

## 一、架构决策全景 (自主最佳决策)
针对之前识别出的 5 大架构摩擦点，本次重构采取三阶段纵深递进方案：
1. 阶段一：底层存储接缝与设备安全控制器下沉 (融合方案 2 + 方案 3 + 方案 4) - [已完成]
   - 在 direct_io.rs 中定义最小 BlockIo trait 接缝，使 I/O 驱动与硬件解耦
   - 在 devices.rs 中构建 StorageDeviceManager 与 StorageTarget，内聚卷卸载与总线隔离
   - 在 fix.rs 中构建 SafePartitionFixer 事务守卫，防止误伤 Disk 0
2. 阶段二：H2W 读写双引擎融合为统一深模块 (方案 1) - [已完成]
   - 创建 src/core/session.rs (H2wSession)，统一文件轮转、对齐缓冲、速率与 ETA 计算
   - 导出统一的 EngineTelemetry，消灭 GUI 双通道轮询
3. 阶段三：GUI 状态调度与视图纯化 (方案 5) - [已完成]
   - 改造 state.rs 与 app.rs，将轮询与调度抽离，使 UI 专注于纯声明式渲染
   - 维护 CLAUDE.md 架构演进记录

## 二、详细任务清单与验收标准

### 任务 1: BlockIo 抽象接缝与 DirectIoFile 适配 - [已通过验收]
- 文件: src/core/direct_io.rs, src/core/probe.rs
- 验收成果:
  1. 定义 trait BlockIo: Send + Sync { fn read_at, fn write_at, fn flush }
  2. DirectIoFile 实现 BlockIo
  3. 提供 MemoryBlockDevice 测试桩结构体，支持脱机模拟扩容盘地址回绕
  4. F3Probe 支持 run_on_device 泛型接缝，包含脱机单元测试

### 任务 2: StorageDeviceManager 与 StorageTarget 深模块 - [已通过验收]
- 文件: src/core/devices.rs
- 验收成果:
  1. 封装 StorageDeviceManager，统一管理 PhysicalDeviceInfo 与 VolumeInfo
  2. 实现 StorageTarget，提供 prepare_for_probe() 方法，内部自动安全执行 FSCTL_LOCK_VOLUME 与 FSCTL_DISMOUNT_VOLUME
  3. 严格禁止 Disk 0 或非移动总线进入 raw probe 流程
  4. VolumeLockGuard RAII 守卫在析构或任务结束时自动 CloseHandle，根治句柄泄露

### 任务 3: SafePartitionFixer 自守卫分区修复 - [已通过验收]
- 文件: src/core/fix.rs
- 验收成果:
  1. 包装 F3FixPlan 为 SafePartitionFixer
  2. 执行前硬性断言 disk_index != 0，并重新核验总线属性 (Zero-TOCTOU)
  3. 提供 dry_run() 预演方法返回脚本与校验结果，UI 界面实时反馈
  4. 安全执行 diskpart 并提供结构化结果与 CREATE_NO_WINDOW 消除黑框

### 任务 4: H2wSession 统一核心测试引擎 - [已通过验收]
- 文件: src/core/session.rs, src/core/mod.rs
- 验收成果:
  1. 支持 H2wMode::Write、H2wMode::Verify 与 H2wMode::WriteAndVerify
  2. 统一管理 AlignedBuffer 与多文件循环，消减 290 行重复代码
  3. 统一算法与速率/ETA平滑计算，发出 H2wTelemetry
  4. 支持后台自动清理测试文件，根除 UI 阻塞卡顿

### 任务 5: GUI 解耦与全景状态接入 - [已通过验收]
- 文件: src/gui/state.rs, src/gui/app.rs
- 验收成果:
  1. state.rs 统一为 h2w_rx，移除分裂的 write_rx/read_rx
  2. app.rs 移除所有直接调用 Win32 FSCTL 卸载的代码，对接 StorageTarget 与 VolumeLockGuard
  3. 削减 65 行双通道轮询样板代码，提升界面可读性与响应速度

### 任务 6: 维护 CLAUDE.md 与最终验证 - [已通过验收]
- 验收成果:
  1. 规范更新 CLAUDE.md 记录架构演进与决策全景
  2. 全量源码遵循无 emoji 规范，无无用引入
  3. 确保本地零编译器调用，全流程符合工程约束
