# 更新日志 (Changelog)

本文件记录 F3-Win 项目的所有重要版本变更。
格式基于 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，并遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

## [0.1.0] - 2026-10-18

F3-Win 首个正式发布版本。将开源闪存扩容盘检测工具 F3 (Fight Flash Fraud) 移植至 Windows 平台，提供单文件绿色、超高性能（饱和物理带宽）、高安全防呆的原生 Rust + Win32 Direct I/O GUI 应用程序。

### Added
- **原生 Direct I/O 读写与校验引擎**：CreateFileW 配置 FILE_FLAG_NO_BUFFERING 与 FILE_FLAG_WRITE_THROUGH，彻底绕过系统文件缓存，消除虚假缓存加速。
- **物理裸盘跳跃探测 (f3probe)**：忠实对齐原版跳跃扇区探测算法，数分钟内精准测出假盘真实物理容量与回绕边界。
- **双轨分区自愈修复 (f3fix)**：支持一键重写 MBR/GPT 安全分区，提供 Win32 API 原生驱动与标准 diskpart 脚本双轨执行机制。
- **实时仪表盘与可视化**：
  - 基于 egui_plot 的 1200 点自适应平滑跟焦测速折线图，精准捕捉 SLC Cache 耗尽真实掉速断崖；
  - 1000 单元扇区健康可视化热力块矩阵（直观渲染 Good、Bad、Changed、Overwritten 状态）。
- **原生 Win32 系统托盘驱动与自愈架构**：
  - 基于 Shell_NotifyIconW 与 TrackPopupMenu 实现，零额外三方依赖；
  - 具备三级自适应容灾加载机制（PE 资源直通、静态内嵌动态构建、系统默认兜底）；
  - 动态显示当前版本号（关于 F3-Win v0.1.0），支持 Win32 原生关于信息对话框弹窗；
  - 注册 TaskbarCreated 广播消息，在资源管理器崩溃重启时无缝自动重载托盘。
- **法庭级维权存证报告中枢**：
  - 自动出具符合《消费者权益保护法》第五十五条标准的惩罚性赔偿主张书；
  - 自动捕捉首发故障扇区微观 Hexdump 现场对照数据；
  - 假盘检测触发时自动在可执行文件同级目录生成防灭失 Markdown 证据档案。
- **智能空间管理**：
  - 一键毫秒级扫描并清理全盘 .h2w 测试碎片；
  - 支持读取校验全部通过后后台自动释放空间；
  - 假盘模式增加证据保护守卫，存在损坏或覆写扇区时硬性拦截删除。

### Performance
- **物理对齐内存管理**：VirtualAlloc 申请 4KB 扇区对齐的环形多缓冲，杜绝额外内存拷贝。
- **SIMD 向量化加速**：针对原版 F3 线性同余 PRNG (4294967311, 17) 与内存比对提供 AVX2 / SSE4.2 / NEON 向量化加速，消除 CPU 吞吐瓶颈。
- **流式生理测速引擎**：基于 Welford 单趟方差与 P² 在线分位数算法，以 O(1) 恒定内存实时更新最高峰值、稳态中位数与速度离散抖动指数。
- **视口图标零开销冷启动**：预编译 64x64 纯原始 RGBA 颜色通道数据，免除运行时图像解码 crate 依赖。

### Security
- **单向雪崩置换防伪机制**：引入 one_way_seed 非线性单向置换，Salt 永不落盘，彻底击穿恶意假盘固件模逆元算力回放作弊。
- **双向对流探针闭环**：高位写专属探针结合 32MB 深度缓存冲刷，高位读回防位黑洞丢弃，低位读回防硬件回绕覆盖。
- **设备安全隔离锁**：基于 IOCTL_STORAGE_QUERY_PROPERTY 严格锁定 USB / SD / MMC 移动介质，自动锁死系统主盘与内部固定磁盘；探测前执行 FSCTL_LOCK_VOLUME 与 FSCTL_DISMOUNT_VOLUME 独占卷卸载。
- **全自动 UAC 管理员提权**：通过 build.rs 注入 /MANIFESTUAC 清单，运行时自动通过 runas 提权，杜绝物理设备 Error 5 Access Denied。

### Branding
- **极简瑞士平面几何 F3 品牌标识**：纯白字母 F 与电光青数字 3 负空间互锁，集成 45 度硅片标准切角。
- **多规格原生图标资产**：覆盖 16x16 至 256x256 全规格 7 档 Windows PE 图标与超清透明底 PNG。

[0.1.0]: https://github.com/Wenaixi/f3-win/releases/tag/v0.1.0
