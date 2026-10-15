# F3-Win 深度架构重构与模块解耦实施计划

> 日期: 2026-10-16
> 执行模式: 内联连续推进 (Inline Execution with Superpowers)
> 目标: 淘汰冗余死代码，消除 UI 层与硬件系统的职责泄漏，构建深层 StorageInventory 与 EngineController

## 任务清单与执行顺序

1. [x] **任务 1: 建立项目领域模型 CONTEXT.md 与实施计划**
   - 编写 CONTEXT.md 领域字典
   - 落实 docs/superpowers/plans/2026-10-16-codebase-architecture-deepening.md
   - 验证: 文件存在且词汇精准
   - 提交: docs: establish domain model in CONTEXT.md and implementation plan

2. [ ] **任务 2: 清除孤立遗留 I/O 模块 (Candidate 1)**
   - 物理删除 src/core/writer.rs 与 src/core/reader.rs
   - 更新 src/core/mod.rs，移除 writer 和 reader 模块声明
   - 验证: 全局 grep 确认无断裂引用
   - 提交: refactor(core): purge dead legacy writer and reader modules

3. [ ] **任务 3: 构建深层存储资产清单 StorageInventory (Candidate 2)**
   - 在 src/core/devices.rs 中实现 StorageInventory
   - 内聚 volumes 与 physical devices 的枚举、映射与安全过滤
   - 暴露 active_target, refresh, select_volume, select_device 等简洁接口
   - 编写单元测试验证内存映射行为
   - 提交: feat(core): implement deep StorageInventory module in devices

4. [ ] **任务 4: 构建深层引擎控制器 EngineController 与 TaskSnapshot (Candidate 3)**
   - 创建 src/core/controller.rs 并在 src/core/mod.rs 导出
   - 实现 TaskSnapshot 只读状态聚合与平滑折线历史
   - 实现 EngineController 统管后台线程、Crossbeam 通道、取消标志与 VolumeLockGuard
   - 编写单元测试验证生命周期状态机
   - 提交: feat(core): implement deep EngineController and TaskSnapshot

5. [ ] **任务 5: 重构 GUI 状态机与界面渲染循环**
   - 改造 src/gui/state.rs: 移除散落字段，由 inventory 与 controller 接管
   - 改造 src/gui/components/device_selector.rs: 消除外部闭包与盘符解析
   - 改造 src/gui/app.rs: poll_telemetry 变为单一 snapshot 轮询，各 Tab 直接与控制器交互
   - 提交: refactor(gui): integrate StorageInventory and EngineController into F3App

6. [ ] **任务 6: 全局静态检查与 CLAUDE.md 项目记忆库同步**
   - 维护 CLAUDE.md 记录架构深化成果
   - 验证代码风格、规范与无编译器铁律遵守
   - 提交: docs: document comprehensive architecture deepening in CLAUDE.md
