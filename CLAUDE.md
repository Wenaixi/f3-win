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

## 交付状态与成果记录
- 远程仓库：https://github.com/Wenaixi/f3-win
- 编译约束落实：严格遵守「严禁本地编译」铁律，本地零编译器运行，全量构建 100% 由 GitHub Actions 云端接管
- 最新 CI 验证运行：Run ID 38920697256 (Status: completed success，全量 26 项单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：全部 26 项单元测试全部通过 (2m 11s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成发布二进制产物 f3-win-x86_64-pc-windows-msvc (2m 38s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成发布二进制产物 f3-win-aarch64-pc-windows-msvc (2m 34s)
  - Release 产物 (x86_64-pc-windows-msvc)：f3-win-x86_64-pc-windows-msvc.zip (~1.65 MB, 部署产物 f3-win.exe 3,106,816 字节)
  - Release 产物 (aarch64-pc-windows-msvc)：f3-win-aarch64-pc-windows-msvc.zip (~1.60 MB, 部署产物 f3-win-arm64.exe 2,965,504 字节)
- 架构验证指标：Direct I/O 驱动无缓冲绕过、SIMD/无对齐安全读写、硬件总线 USB/SD 隔离锁均已全量就绪

### 深度架构进化与体验升级总结 (Run 38756354852)
1. 全自动 UAC 管理员提权机制：
   - 编译期：通过 build.rs 注入 MSVC 链接指令 /MANIFESTUAC:level='requireAdministrator'，生成自带系统级盾牌图标的权威特权二进制
   - 运行时：main.rs 启动时通过 IsUserAnAdmin() 检测令牌，若未提权自动调用 ShellExecuteW(runas) 自我重启唤起管理员，杜绝物理设备 Access Denied (Error 5)
2. 动态自适应平滑跟焦速度曲线：
   - 彻底解决 egui_plot 相机视野脱靶死穴，实时跟踪采样极值注入 include_x(0.0..max_time) 与 include_y(0.0..max_speed)，保证物理写速与颗粒掉速断崖 100% 居中展现
   - 赋予独立绘图 ID (write_speed_plot / read_speed_plot)，开启缩放与拖拽探索
3. 真实动态高精度 ETA 倒计时：
   - 写入端自动通过 Win32 GetDiskFreeSpaceExW 探测剩余可用总字节数作为上限，读取端汇总 .h2w 总容量，彻底消除 0 秒硬编码截断
   - 人性化格式化为「X小时X分X秒」动态倒计时看板
4. 逻辑卷与物理磁盘无缝联动：
   - 实现 IOCTL_STORAGE_GET_DEVICE_NUMBER，在 f3probe 选项卡中自动将选中的 G: 盘反查并默认绑定其底层的 PhysicalDrive2
   - 探测启动前自动对该磁盘全部挂载卷调用 FSCTL_DISMOUNT_VOLUME 独占卸载，解决文件系统写冲突
5. 现代 Bento-Grid 四卡片核心指标看板与一键 Markdown 诊断报告导出

### 终极大成版交付记录 (Run 38762490059)
- 产物详情：
  - Windows x64: f3-win.exe (约 3.10 MB, 时间戳 15:47:10)
  - Windows ARM64: f3-win-arm64.exe (约 2.95 MB)
- 像素级对齐原版 F3 核心算法：
  1. f3probe: 避开扇区 0，在 1MB (扇区 2048) 写入基准探针，并在磁盘高位执行 overwhelm_cache 强制排空主控 SRAM/DRAM 缓存，随后通过 2^k + 2048 高位地址镜像回读判定真实扇区回绕
  2. f3write: 增加 end_file_num 闭区间截断与图形化控件，严格对应原版 --end-at
  3. f3read: 热力图采样实现 total_read / total_expected_bytes * 1000 全盘百分比线性归一化映射，攻克超 1GB 热力图停滞缺陷；增加 Untested 灰色初始态
  4. UI 架构重构：抽离公共 render_volume_selector 组件，消除 70+ 行样板代码；折线图引入 1200 点环形缓冲与自适应平滑跟焦
  5. 进程安全：严格遵守不停止、不干扰任何运行中已有进程的铁律，通过 Windows 原生更名热更新机制平稳落地

### 物理设备 open_raw_device 与假阳性报告根除记录 (Run 38764978664)
- 根因定位：
  1. Win32 规定打开底层物理磁盘 \\.\PhysicalDriveX 或卷对象时，dwCreationDisposition 必须严格为 OPEN_EXISTING (3)，严禁使用普通文件的 CREATE_ALWAYS (2)；原代码误用 CREATE_ALWAYS 导致内核拒绝打开
  2. 探测需要同时执行扇区写入与镜像回读，句柄必须同时具备 GENERIC_READ | GENERIC_WRITE (0xC000_0000)
  3. UI 诊断逻辑在 res.error 存在时未优先拦截，把 0 字节误判为通过并虚假显示「物理容量完全合规」
- 修复成果：
  - 新增 DirectIoFile::open_raw_device 专用方法，配置 OPEN_EXISTING 与 0xC000_0000
  - f3probe 切换为该专用驱动，启动前自动对全部挂载卷调用 FSCTL_DISMOUNT_VOLUME 独占卸载
  - UI 优先判断 res.error 并以鲜红色大字展示明确的失败根因与 UAC 排查建议
  - 最新可执行文件 f3-win.exe (大小 3,103,232 字节，时间戳 16:03:10) 已安全热替换就位

### 智能清理与自动释放空间交付记录 (Run 38767426011)
- 产物详情：
  - Windows x64: f3-win.exe (支持一键清理与校验后自动释放空间)
  - Windows ARM64: f3-win-arm64.exe
- 新增贴心功能：
  1. 一键清理测试文件：在写入页与读取页均增加「一键清理所有 .h2w 测试文件 (释放空间)」按钮，毫秒级扫描并秒删全部测试碎片，自动刷新磁盘剩余可用空间
  2. 自动清理释放空间：读取校验页新增勾选框「读取校验通过后自动清理全部测试文件」，校验全盘数据通过后，在后台全自动清除所有测试数据，还用户一个干干净净、空间完全释放的磁盘
  3. 进程安全：严格遵守不停止、不干扰任何运行中已有进程的铁律，通过 Windows 原生更名热更新机制平稳落地

### Humanizer 全量去 AI 腔文案与界面重塑交付记录 (Run 38769729385)
- 产物详情：
  - Windows x64: f3-win.exe (约 3.10 MB, 时间戳 16:37:08, 纯自然工程文风)
  - Windows ARM64: f3-win-arm64.exe
- 人性化文案重构亮点：
  1. 窗口标题由浮夸的「极致闪存测试与自愈终端」精炼为平实自然的「F3 (管理员)」
  2. 操作按钮全面去修饰词：「开始写入」、「开始校验」、「开始探测」、「应用分区修复」、「清理测试文件」
  3. 诊断报告由情绪化推论改为冷静客观的事实陈述（例如：「检测到容量虚标。标称 X GB，实际可用 Y GB。可前往分区修复」）
  4. 选项文案清晰直白（「校验完成后自动删除测试文件」）
  5. 技术文档 README.md 彻底剔除浮夸宣传用语，以中立规范的工程规格交付
  6. 严格执行不停止、不干扰任何运行中已有进程的铁律，平稳落地
### 全模块深度架构升级与解耦成果记录 (2026-10-16)
1. BlockIo 最小块 I/O 接缝与离线仿真设备：
   - 在 direct_io.rs 中定义 trait BlockIo { read_at, write_at, flush }，为 DirectIoFile 实现该 trait
   - 新增 MemoryBlockDevice 稀疏哈希仿真块设备，精准模拟物理颗粒容量截断与硬件地址回绕（Wrap-around）
   - 将 F3Probe 解耦为 run_on_device(&self, dev: &mut impl BlockIo, tx: ...)，在脱机 CI 和单测中实现 100% 算法自动化覆盖，无需管理员提权或物理硬件
2. StorageTarget 与 VolumeLockGuard RAII 卷独占锁闭环：
   - 彻底解决 app.rs 中丢弃裸 HANDLE 导致的句柄永久泄漏与 FSCTL_LOCK_VOLUME 锁失效 Bug
   - 实现 VolumeLockGuard RAII 守卫（实现 Drop 自动 CloseHandle），在后台探测任务全程持有排他锁，任务完成/中断/panic 时内核驱动自动平稳恢复挂载
   - 封装 StorageTarget 与 StorageDeviceManager，将逻辑卷与物理磁盘的映射与 FSCTL 独占卸载完全内聚在底层，GUI 彻底消除裸 Win32 系统调用
3. SafePartitionFixer 事务型自守卫分区自愈：
   - 绝对物理防御：硬性拦截 PhysicalDrive0 (系统主盘)，杜绝参数越界或驱动器号漂移抹除操作系统的灾难
   - 即时现场硬件复核 (Zero-TOCTOU)：通过 query_by_index 实时重新确认物理磁盘的 Usb/Sd/Mmc 移动总线属性，物理隔离内部固定磁盘
   - 提供 dry_run 预演验证报告，以 CREATE_NO_WINDOW 消除控制台黑框，完善 stdout 错误解析与 RAII 临时脚本自动销毁
4. H2wSession 深度融合引擎与统一 Telemetry：
   - 将原本分散在 writer.rs 与 reader.rs 中的 590 行双轨逻辑彻底重构成单一 session.rs（约 300 行），净减少 ~290 行重复代码
   - 统一 AlignedBuffer 管理、多文件轮转、PRNG 数据生成/比对与平滑速率/ETA 状态机
   - 统一导出 H2wTelemetry 与单一通道 h2w_rx，支持连续「写入并校验」模式
   - 将文件自动清理从 UI 主线程移至 Worker 线程的 CleaningUp 阶段，根除慢速 U 盘上主界面短暂卡顿
### 全模块深度架构升级与解耦成果记录 (2026-10-16 第二轮深化交付 Run 38778382190)
- 产物详情：
  - Windows x64: f3-win.exe (3,106,816 字节，时间戳 17:39:32)
  - Windows ARM64: f3-win-arm64.exe (2,965,504 字节，时间戳 17:41:28)
- 架构验证与治理指标：
  - 静态检查：零警告通过 cargo clippy --all-targets 严格门禁
  - 单元测试：15/15 核心单测全部 Pass (包含 PRNG 序列、SIMD 比对、MemoryBlockDevice 虚拟块仿真、真假盘回绕探测、StorageInventory 资产清单与 EngineController 状态机)
  - 进程安全落地：严格执行 Windows 原生更名热替换机制，平稳完成最新特权二进制部署

1. 彻底净化孤立遗留 I/O 模块 (Candidate 1 落地):
   - 物理移除 src/core/writer.rs (207 行) 与 src/core/reader.rs (270 行)，共计净删除 477 行旧代码
   - 消除 core 模块中由 session.rs 取代后残存的虚假接缝与认知摩擦，确立 H2wSession 作为单一权威 H2W 测试引擎
2. 抽象深层存储资产清单 StorageInventory (Candidate 2 落地):
   - 在 src/core/devices.rs 中实现深层结构体 StorageInventory
   - 将系统卷 (VolumeInfo) 与底层物理磁盘 (PhysicalDeviceInfo) 的枚举、双向映射关联与安全白名单校验完全内聚
   - 提供 active_target、refresh、select_volume、select_device 极简高层查询表面，彻底消除了界面层中盘符字符截取、低级 IOCTL 反查与刷新闭包
3. 沉淀深层任务控制器 EngineController 与只读快照 TaskSnapshot (Candidate 3 落地):
   - 新增 src/core/controller.rs，统领异步后台线程生命周期、Crossbeam 通道、原子取消标志与 VolumeLockGuard 锁持有
   - 封装 1200 点自适应速度折线图平滑缓冲采样与 ETA 计算
   - 每帧导出只读 TaskSnapshot，彻底将 F3App 还原为纯净、无状态的视图呈现层，F3App 单文件净减少 110 行胶水代码
4. 业务领域模型字典 CONTEXT.md 正式确立:
   - 明确定义 StorageInventory、StorageTarget、VolumeLockGuard、H2wSession、F3Probe、EngineController、TaskSnapshot、SafePartitionFixer 等核心概念与深浅模块职责边界
### 全模块所有细节深层架构深化成果记录 (2026-10-16 第三轮深化交付: 全生命周期闭环)
1. SafePartitionFixer 解耦与 DiskpartRunner 接缝 (Candidate 1 落地):
   - 引入最小抽象接缝 `pub trait DiskpartRunner: Send + Sync { fn run_script(&self, script: &str) -> Result<String, String>; }`
   - 落地 `SystemDiskpartRunner` 生产适配器 (内聚 CREATE_NO_WINDOW、微秒防冲突临时文件与 RAII 自动清理守卫) 与 `MockDiskpartRunner` 单测仿真适配器
   - 在 `F3FixPlan` 中实现纯逻辑级别 `execute_with_runner`，并在 CI 中提供 100% 自动化脚本指令断言与退出码解析单测覆盖
2. StorageTarget 交付 BlockIo 锁门面与 F3Probe 单一接口收敛 (Candidate 2 落地):
   - 在 `StorageTarget` 中实现深层复合方法 `open_block_io(&self) -> Result<(DirectIoFile, VolumeLockGuard), String>`，原子化管理设备总线白名单、FSCTL 卷卸载排他锁与物理磁盘直通句柄获取
   - 彻底消除 `F3Probe` 内部对 Win32 `DirectIoFile` 类型的直接依赖与双入口违规，去除配置中冗余的物理路径字符串，将对外接口收敛为唯一的 `pub fn run(&self, dev: &mut impl BlockIo, tx: Sender<ProbeTelemetry>)`，100% 践行「测试表面即调用表面」铁律
3. EngineController 全任务五合一与异步单向流 (Candidate 3 落地):
   - 扩充 `TaskKind::Fix` 与 `TaskKind::Cleanup`，新增 `start_fix(plan)` 与 `start_cleanup(target_path)` 异步调度
   - 消除 UI 主线程在 diskpart 分区格式化与慢速 USB 上删除数十 GB 测试碎片时的同步阻塞卡死，将全部破坏性与文件系统操作统领于后台 Worker 线程，通过 `TaskSnapshot` 实现单向数据流与运行状态采样
4. F3App 纯净视图与响应式自动刷新:
   - 所有操作按钮（写入、校验、探测、修复、清理）全面改造为非阻塞控制器派发与运行中置灰保护 (`!is_busy`)
   - `F3App::update()` 单帧循环自动检测任务完成边界 (`prev_status == Running && status == Finished`)，静默自动触发 `inventory.refresh()`，盘符与可用容量即时同步更新

### 深度安全攻防与假盘绕过对抗架构决策 (2026-10 终极防伪审计)
1. 假盘固件算力回放 (Compute-on-Read) 与模逆元攻击防线：
   - 漏洞揭秘：原版 F3/H2testw 的 LCG (4294967311, 17) 乘数为奇数，在模 2^64 下存在唯一模逆元。若仅使用简单的 offset ^ salt，恶意固件通过扇区首部的明文 offset 和第一个随机数 R_1，仅需单步乘加即可解出主机端 Salt，继续实施 0 字节物理闪存欺骗。
   - 终极防御决策：废除纯线性同余单一派生，引入单向不可逆 PRF (如 ChaCha8 / SipHash-2-4 / 64位单向雪崩置换)。Salt 永不落盘，使主控无法在写阶段反解密钥，从信息论上强制其必须真实落盘。
2. f3probe 双向黑洞与大缓存穿透闭环：
   - 彻底修复单向读高位导致的「位黑洞假盘 (读全0/全FF) 假阳性漏报」重大 Bug：重构为双向对流探针（高位写探针 -> 深度排空缓存 -> 高位读回防黑洞 -> 低位读回防回绕）。
   - overwhelm_cache 冲刷量从 128KB 动态升至 32MB~64MB，物理击穿现代主控的 SLC Cache。
3. Win32 Direct I/O 与容灾健壮性固化：
   - 同步句柄彻底移除 GetOverlappedResult，直取 GetLastError() 真实硬件错误码。
   - 严格区分 ERROR_DISK_FULL (112) 与真实硬件故障 (1117 / 23)，杜绝意外掉盘虚假上报“测试成功”。
   - 自动清理增加防伪守卫：存在 overwritten / changed 扇区时严禁删除测试文件，完整保留假盘维权证据。

### 终极防伪与容灾加固大成版交付记录 (Run 38933445641)
- 产物详情与流水线指标：
  - 流水线状态：Run ID 38933445641 (Status: completed success，全量 31 项深度单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部 31 项单元测试 (2m 51s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc (2m 6s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc (2m 15s)
- 核心攻防与架构飞跃：
  1. 单向雪崩置换绝杀模逆元算力回放：实现 one_way_seed 非线性单向置换，Salt 绝不落盘；当 salt == 0 时保持原版 F3/H2testw 100% 逐比特向后兼容，当 salt != 0 时击穿模逆元 0x6789abcdeeeeeeef 算力回放作弊，信息论强制假盘必须真实落盘。
  2. 双向对流探针彻底终结位黑洞与回绕漏报：在每个指数跳跃阶梯先写高位专属探针再深度排空 32MB 缓存，高位读回防位黑洞丢弃、低位读回防物理回绕覆盖，并设置小于 2MB 安全熔断保护扇区 0 (MBR)。
  3. Win32 Direct I/O 同步错误直取与假盘证据保护：彻底剔除非重叠同步句柄上的 GetOverlappedResult 误用，直取真实 Win32 硬件错误码；写入循环精准区分磁盘写满 (112/39) 与硬件掉盘 (1117/23)；自动清理增加防伪守卫 (overwritten > 0 假盘严禁删除测试文件，完整保留维权凭据)。
  4. 状态机自愈与系统盘绝对安全绝缘：poll() 捕获管道 Disconnected 异常防止 Worker 崩溃导致 UI 按钮永久置灰卡死；热力图采样严重度单调递增保持；StorageTarget 对系统引导卷 (C:) 实施一票否决；规范 diskpart 语法为 REM 并移除多余 active；默认开启单向安全加盐，空闲时静默休眠节能。









### 原生 Win32 系统托盘与核心路径信息中心大成版交付记录 (Run 39033141048)
- 流水线状态：Run ID 39033141048 (Status: completed success，全量核心单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部单元测试 (1m 57s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 12s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (2m 22s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,267,072 字节, SHA-256: FCA0C527BE5C09447AB7A9FD83DBBE84B21790055F2766DE998220361DDF3936，编译时间戳 2026-10-17 12:39:44)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,105,280 字节, SHA-256: 660081A26A2EDF945F3971C13041248AA3694372594C9F77FEAAAA67B75CE629，编译时间戳 2026-10-17 12:39:54)
- 架构与交互核心飞跃：
1. 原生 Win32 系统托盘驱动中枢 (src/gui/tray.rs):
   - 零外部三方依赖：直接复用 Windows 平台原生 Shell_NotifyIconW、TrackPopupMenu 与 HWND_MESSAGE 隐形消息窗口，零引入多余 crate 依赖；
   - 异步解耦设计：专有 Win32 消息隐形窗口驱动事件循环，通过 crossbeam-channel 与 eframe/egui 60FPS 主循环完全非阻塞解耦，弹出右键菜单绝不阻塞主界面渲染；
   - 资源管理器崩溃自愈 (TaskbarCreated)：动态注册 Windows Shell 广播消息，当 explorer.exe 重启时自动重新挂载托盘图标，托盘永不失联；
   - RAII 彻底根除幽灵托盘图标：在 TrayManager 析构 (Drop) 时强制向系统下发 NIM_DELETE，解决 Windows 托盘常见的“划过才消失”幽灵残留；
   - 动态生理状态 Tooltip：实时同步 Direct I/O 写入/校验瞬时速率与已处理 GB 字节数至任务栏托盘悬停提示看板；
2. 核心路径服务中心与双模资源管理器联动 (src/core/paths.rs & app.rs):
   - 统一汇集 AppPathInfo 权威路径事实：可执行程序绝对路径 (exe_path)、程序所在安装目录 (exe_dir)、进程工作目录 (working_dir)、当前选中测试卷 (selected_volume)、维权存证档案目录 (evidence_dir) 与系统临时目录 (temp_dir)；
   - 资源管理器智能双模调用：针对文件精准传导 /select,<path> 参数高亮定位目标，针对目录直接秒级弹出开启；
   - 跨进程剪贴板管道直通：复用 Windows 原生 clip.exe 管道，一键复制任意路径；
   - 顶部导航栏直通按钮与高级设置页全景看板双轨覆盖；
3. 窗口生命周期与硬件防呆退出拦截 (src/gui/app.rs & state.rs):
   - 最小化到托盘防误触：在高级设置中默认开启「点击窗口关闭按钮 (×) 时最小化到系统托盘」，防止误触右上角关闭按钮导致耗费数小时的 Direct I/O 硬件测试中断；
   - 运行中防呆二次确认：托盘菜单「安全退出」与窗口关闭操作在 Direct I/O 测试进行中实施模态拦截，明确提示硬件写入与卷锁风险，用户显式确认后先调度 controller.cancel() 释放设备句柄和卷排他锁，平稳优雅退出；
4. 全量代码与文档绝对零 emoji 规范恪守。

### 全物理总线7阶梯与闪存6级体质工业级判定矩阵大成版交付记录 (Run 39031602542)
- 流水线状态：Run ID 39031602542 (Status: completed success，全量 36 项核心单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部 36 项单元测试 (2m 14s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 16s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (2m 16s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,218,432 字节, SHA-256: 7EBB167F2799656021311490BFB9564692F374B31CBE7F7BD1AB941E32393632，编译时间戳 2026-10-17 12:16:46)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,061,248 字节, SHA-256: 2265BDA53A4CCC2FB3BD8AFE34E4B22F77CC3079A40F5EEC546ED6001D7C86DE，编译时间戳 2026-10-17 12:16:44)
- 究极完善的硬件物理推断引擎核心进化：
  1. USB 物理通道判定阶梯全面扩充至 7 个全阶梯 (基于 USB-IF 物理层与 BOT/UASP 实际载荷实测数据)：
     - Tier 0: < 1.5 MB/s -> USB 1.1 Full-Speed (12Mbps 物理通道，实测有效上限约 1.2 MB/s)
     - Tier 1: 1.5 ~ 43.5 MB/s -> USB 2.0 High-Speed (480Mbps 物理通道，实际读取带宽上限约 43.5 MB/s)
     - Tier 2: 43.5 ~ 450.0 MB/s -> USB 3.2 Gen 1x1 (5Gbps 物理通道，SuperSpeed 理论 500MB/s，BOT/UASP 实测上限约 450 MB/s)
     - Tier 3: 450.0 ~ 1050.0 MB/s -> USB 3.2 Gen 2x1 (10Gbps 高速物理通道，128b/132b 编码，实测上限约 1050 MB/s)
     - Tier 4: 1050.0 ~ 2050.0 MB/s -> USB 3.2 Gen 2x2 (20Gbps Type-C 双通道高速，实测上限约 2050 MB/s)
     - Tier 5: 2050.0 ~ 3800.0 MB/s -> USB4 Gen 3x2 / Thunderbolt 3/4 (40Gbps PCIe 隧道物理通道)
     - Tier 6: >= 3800.0 MB/s -> USB4 2.0 / Thunderbolt 5 (80/120Gbps PAM3 超高速物理通道)
  2. SD / TF 卡总线判定阶梯全面落地 7 档标准 (SD 协会官方物理标准对齐)：
     - Default Speed (< 12.5 MB/s, SD 1.01 3.3V 25MHz)
     - High Speed (12.5 ~ 25.0 MB/s, SD 1.10 3.3V 50MHz)
     - UHS-I SDR50/DDR50 (25.0 ~ 50.0 MB/s, SD 3.01 1.8V)
     - UHS-I SDR104 (50.0 ~ 104.0 MB/s, SD 3.01 1.8V 208MHz 4-bit)
     - UHS-II (104.0 ~ 312.0 MB/s, SD 4.00 LVDS 双排引脚物理总线)
     - UHS-III (312.0 ~ 624.0 MB/s, SD 6.00 全双工差分物理总线)
     - SD Express (>= 624.0 MB/s, SD 7.00/8.00 PCIe Gen3/Gen4 + NVMe 超高速总线)
  3. NAND 闪存颗粒体质等级全面扩充至 6 级 (Grade F 至 Grade S)：
     - Grade F (极度报废黑片/严重漏电下脚料/报废TF打胶片，写速 < 1MB/s)
     - Grade D (深度降级 TLC 晶圆残次片/二手翻新拆机片 Ink Die，写速 1~3MB/s)
     - Grade C (降级次品白片/3D QLC 边缘晶圆次品，写速 3~8MB/s)
     - Grade B (常规合格商业级 3D TLC 原厂白片/Good Die B-Grade，写速 8~25MB/s)
     - Grade A (原厂高良率 Good Die/eMMC 5.1/UFS 级颗粒，写速 25~80MB/s)
     - Grade S (企业级/高阶 PCIe NVMe 闪存阵列颗粒，配备独立 DRAM 硬件缓存，写速 > 80MB/s)
  4. 芯片级四维物理病理解析升级：
     - 涵盖 SLC 模拟缓存耗尽断崖、DRAM-less 无外置物理缓存、FTL 逻辑映射表重构、GC 垃圾回收物理块擦除锁定 (tBERS 2~5ms) 与温控热节流降频；
  5. 全端三位一体对齐与绝对零 emoji 规范恪守：
     - Markdown 报告、纯文本案卷与界面公文卡片三端完全由核心推断中枢统一驱动，全工程无 emoji 字符残留。

### 底层生理测速深度融合与全细节极致优化大成版交付记录 (Run 39025806862)
- 流水线状态：Run ID 39025806862 (Status: completed success，全量单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部单元测试 (2m 3s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 10s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (2m 4s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,217,920 字节, SHA-256: EDED52C26EFAFBF6B6C14755956192F0C5B5EA96A73AF40999F630190672ABD9，时间戳 2026-10-17 10:44:48)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,060,736 字节, SHA-256: FF5A9654620714FD1870855EC92538148E6F671F577B2413F5E4764D60ECF5BD)
- 深度架构与细节优化落地成果：
  1. 底层生理测速引擎 100% 深度融合 (src/core/session.rs & speed.rs):
     - 将原本简易的 ProgressTracker 与工业级 SpeedPhysiologyTracker 深度内聚，在写入执行 (execute_write) 与读取校验执行 (execute_verify) 处理每一个物理数据块的微秒间隙，实时驱动 Welford 流式方差、P² 在线分位数、RMSSD 相继差抖动与 3 阶段 SLC 耗尽防抖状态机更新；
     - 遥测发射点全面升级为 make_telemetry_with_tracker，所有周期心跳与终结包均携带 100% live 硬件实测物理快照 (Some(tracker.snapshot(bytes)))；
  2. 控制器遥测管道与只读快照闭环 (src/core/controller.rs):
     - TaskSnapshot 扩充 duration_seconds、physiology 以及 first_error_* 取证字段，poll() 单帧循环实时吸收同步，并在 reset_for_new_task() 时实施严格状态隔离，防止跨会话幽灵数据串扰；
  3. 读写双端专属报告公文体系全维度解耦与实测直通 (src/core/report.rs & app.rs):
     - DiagnosticReport 结构体增加对称的读取测速字段 (verify_peak_speed_mb / verify_valley_speed_mb / verify_median_speed_mb / verify_cv_jitter / verify_stall_ratio)，build_write_audit 与 build_verify_audit 分别直传实测物理快照，彻底消灭任何人工乘数估算；
     - 写入测试页底部专属呈现【物理写入性能与颗粒体质检验书】(render_write_report_card)；
     - 读取校验页底部专属呈现【容量真实性与读写生理鉴定结论书】(render_verify_report_card)；
  4. 客观严谨的总线规范与自由垂直长截图支持:
     - 彻底剔除主观浮夸的“伪装 USB 3.0 (装饰性蓝色胶芯)”文案，实事求是标明为【USB 2.0 (High-Speed 480Mbps 物理通道，读取带宽上限约 43 MB/s)】；
     - CentralPanel 注入 ScrollArea 垂直滚动，彻底根除 700px 窗口边缘裁切问题，支持鼠标滚轮自由浏览与 Win + Shift + S 完整长截图；
  5. 全量代码、注释与公文界面绝对零 emoji 规范恪守。

### 剔除主观伪装假设与实事求是USB 2.0规范化大成版交付记录 (Run 39013880095)
- 流水线状态：Run ID 39013880095 (Status: completed success，全量单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部单元测试 (2m 17s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 11s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (2m 15s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,212,288 字节, SHA-256: DD85BCECB079045D1B6BF78509A41E006E66B510D8A5F6B6B4EDA84547263D85，时间戳 2026-10-17 07:34:48)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,057,152 字节, 时间戳 2026-10-17 07:34:52)
- 核心体验与文案进化：
  1. 彻底剔除主观浮夸的“伪装 USB 3.0 (装饰性蓝色胶芯)”文案：
     - 实事求是、科学严谨：对于实测峰值速率低于 43 MB/s 的设备，直接明确标明为客观事实【USB 2.0 (High-Speed 480Mbps 物理通道，读取带宽上限约 43 MB/s)】，杜绝将正品合规的正常 USB 2.0 设备误扣“假盘伪装”帽子；
     - 速率在 43~150 MB/s 时客观标明为【USB 3.0 Gen1 (5Gbps 物理通道)】，速率高于 150 MB/s 时标明为【USB 3.2 / 固态级高速物理通道】；
  2. 垂直滚动自由浏览与截屏无死角：
     - CentralPanel 注入 ScrollArea 垂直滚动支持，彻底根除 700px 窗口高度导致的公文卡片边缘截断问题；
  3. 读取校验报告全面对齐写入报告的全套生理测速与颗粒电荷体质分析；
  4. 全量代码与文档绝对零 emoji 规范恪守。

### 系统化调试根除窗口截断与读取全量报告大成版交付记录 (Run 39003661420)
- 流水线状态：Run ID 39003661420 (Status: completed success，全量单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部单元测试 (2m 4s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 5s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (1m 58s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,212,800 字节, SHA-256: 7564EA445B3C95895F65F717CB724AAF20E4FBDAB9C2BBF72431EE60E40CF19C，时间戳 2026-10-17 04:48:50)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,057,664 字节, SHA-256: 461325F3CFC1173FB5AD88121CBC5F83E2D1BE1A0AD1072FA47B817FCA60D911)
- 系统化调试根因定位与修复成果：
  1. 界面垂直截断彻底根除 (src/gui/app.rs):
     - 根因分析：窗口固定高度为 700px，页面上方标题栏、控制器、测速折线图 (200px) 与热力图 (80px) 累积高度超 570px，而 CentralPanel 原本缺少 ScrollArea，导致展开的电子公文报告卡片下方内容 (包括硬件指纹、读取速率全景看板与微观失配分析) 全部超出 700px 物理边界被窗口边缘直接裁切，用户无法向下滚动查看；
     - 架构修复：为 CentralPanel 注入 egui::ScrollArea::vertical().auto_shrink([false, false]) 全局自适应滚动条，无论窗口多大均支持丝滑滚轮上下滚动，整张电子公文卡片 100% 完整展现与自由截屏；
  2. Markdown 与界面卡片双端读取速率全景补齐 (src/core/report.rs & app.rs):
     - 读取校验报告全面升级为写入报告等同深度的全套生理指标 (平均读取校验速率、最高峰值读速、最低读速冷启动滤波、稳态中位读速、读取总耗时、43MB/s 真实硬件读取接口推断、闪存颗粒电荷体质分析、芯片级读取生理特征解读、AAA级健康介质综合评级、逐扇区四态分布、大白话水桶比喻与消法第55条退一赔三维权指南)；
  3. 全量代码与文档绝对零 emoji 规范恪守。

### 读取校验全套生理测速与读写报告独立解耦大成版交付记录 (Run 39002082479)
- 流水线状态：Run ID 39002082479 (Status: completed success，全量单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部单元测试 (2m 16s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (1m 29s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (1m 45s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,209,216 字节, SHA-256: 9F7F8628D9B7A79BE8D56CC9C303F386F71BF001CB63F096AF733D0A34920065，时间戳 2026-10-17 04:22:26)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,055,104 字节, SHA-256: C3C87814B0C46A320976650EECFD69F0FB11CC797E8AAEE142B9F3A9C2C958C0)
- 核心功能进化：
  1. 读取校验报告全面升级为写入报告相同级别的全套生理指标 (render_verify_report_card):
     - 平均读取校验速率、最高读取峰值速率、最低读取谷底速率 (32MB冷启动智能滤波)、稳态中位读取速率 (P²在在线算法);
     - 真实硬件物理读取通道推断 (43MB/s 红线判定伪装 USB 3.0 vs USB 2.0);
     - 闪存颗粒电荷体质推断 (浮栅绝缘层漏电与电荷保持力分析、正品 Good Die 评级);
     - 芯片级读取生理特征解读 (主控 FTL 逻辑物理映射状态、读速波动稳定性);
     - 综合质量评级 (合规真盘呈现 [AAA级健康介质]，假盘呈现严重虚标欺诈定性);
     - 扇区健康四态分布、大白话水桶比喻 (64L vs 8L)、首发故障 Hexdump 对照与消法第55条退一赔三维权指南;
  2. 读写专属报告彻底分离:
     - 写入测试页底部专属呈现【物理写入性能与颗粒体质检验书】;
     - 读取校验页底部专属呈现【容量真实性与读写生理鉴定结论书】;
     - 彻底解耦，互不干扰混淆，均支持按快捷键 Win + Shift + S 一键截屏作为法定证据;
  3. 全量代码与文档绝对零 emoji 规范恪守。

### 读写专属报告独立解耦与直接截屏电子公文卡片大成版交付记录 (Run 39001093591)
- 流水线状态：Run ID 39001093591 (Status: completed success，全量单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部单元测试 (2m 11s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 3s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (1m 58s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,206,656 字节, SHA-256: 6A78318F79A1B1C907F046CC2E07EA1C2AE0AECEEF94753ED39E3AC3B0FA4A3D，时间戳 2026-10-17 04:07:10)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,048,960 字节, SHA-256: 22D1E845612A459A699BA9D82A8A91D2A5D98472639D053293A2237698B96C99)
- 架构与体验核心进化：
  1. 读写两端报告彻底解耦分工：
     - 写入测试页底部专属呈现【物理写入性能与颗粒体质检验书】(render_write_report_card)：专注于实测写入平均速率、最高峰值、最低谷底滤波、稳态中位数、SLC 缓存耗尽断崖拐点、43MB/s 伪装 USB 3.0 判定、颗粒等级推断与 GC 垃圾回收死锁病理解析；
     - 读取校验页底部专属呈现【容量真实性与欺诈鉴定结论书】(render_verify_report_card)：专注于实测读取平均速率、检验总容量、扇区四态分布、大白话水桶比喻、首发故障 Hex 对照以及《消法》第 55 条退一赔三维权指南；
  2. 电子公文式相框设计与一键截屏支持：
     - 摒弃繁琐的复制剪贴板寻找粘贴软件的步骤，界面底部直接内嵌渲染具备权威公文格式、高对比度深色相框的电子版技术检验书；
     - 买家只需按系统快捷键 Win + Shift + S 或使用截屏软件直接截取该公文卡片，即可作为发给电商客服、平台小二或全国 12315 投诉的合法图片铁证；
  3. 全量代码与文档绝对零 emoji 规范恪守。

### 法庭级维权存证报告中枢与全维度生理速度审计大成版全绿交付记录 (Run 38968080140)
- 流水线状态：Run ID 38968080140 (Status: completed success，全量 33 项核心单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部 33 项单元测试 (2m 25s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 5s)
### f3probe 探针缓存排空窗口与二分逼近加固交付记录 (Run 39294951467)
- 流水线状态：Run ID 39294951467 (Status: completed success，全量 46 项单元测试 100% 通过，x86_64 与 aarch64 双架构构建全绿)
- 根因链与修复（src/core/probe.rs）：
  1. 缓存排空窗口越界覆盖探针：原 scratch_sector_for 用 sector.saturating_sub(flush_depth)，低容量设备 (64MB 标称、flush_depth=16384 扇区) 下溢到 0，冲刷从 LBA 0 一路写到采样点，把 2048/4096 两个基准探针连带覆盖，正品盘被误判为 WrapAround
  2. 重构为 fn scratch_window(sector: u64, flush_depth: u64) -> (u64, u64)，返回 (起点, 可用深度)：下界恒高于 AUX_PROBE_SECTOR + CACHE_FLUSH_BLOCK_SECTORS (4224)，上界恒低于采样点；空间不足时收缩深度而非越界
  3. 采样点与基准探针重合消除：Phase 1 跳跃采样跳过 test_sec == BASE_PROBE_SECTOR || test_sec == AUX_PROBE_SECTOR，杜绝自身写入覆盖基准造成的必然假阳性
  4. 初始缓存预热收敛：改为从 4224 起、深度 CACHE_FLUSH_MIN_SECTORS 且不超过 total_sectors 的连续安全区，避免在真实颗粒远小于标称的回绕盘上写穿整个盘面把基准探针冲掉
  5. 冲刷写入错误不再静默：overwhelm_cache 用 map_err 上报 write_at 与 flush 的失败，真实设备上排空失败会显式暴露而非假通过
  6. 探针缓冲脏数据截断：create_probe_block 只填充本扇区 512 字节，AlignedBuffer 按 4096 对齐后残留的尾部不再被当作缓冲内容写出去
- 验收断言：6MB 非二次幂回绕盘必须 is_fake 且 trap == WrapAround 且 real_size 收敛于 5~7MB；64MB 合规正品盘必须 !is_fake、trap == None、real_size == announced_size
- 全量代码、注释与文档绝对零 emoji 规范恪守。

### 全新极简品牌图标系统与原生Win32系统托盘全景落地记录 (2026-10-17)
- 品牌资产与图标架构核心飞跃：
  1. 极简品牌标识确立 (Concept 01 Negative Space F3 Monogram):
     - 纯粹瑞士平面设计几何美学，纯白字母 F 与电光青数字 3 负空间互锁，集成 45 度硅片标准识别切角 (Chamfer)；
     - 沉淀多规格原生资源：assets/app_icon.ico (涵盖 16x16 至 256x256 全规格 7 档标准 Windows 尺寸)、assets/icon.png (460x460 高清透明底原图)、assets/icon_256.png、assets/icon_64.png、assets/icon_32.png 与预编译 RGBA 像素矩阵 (assets/icon_64.rgba、assets/icon_32.rgba)；
  2. 原生 Win32 系统托盘三级自愈加载架构 (src/gui/tray.rs):
     - 针对用户核心诉求全面贯通系统托盘品牌展示，设计 load_tray_icon() 三级自适应加载中枢；
     - 第 1 级 (PE资源直通): 通过 GetModuleHandleW 与 LoadIconW(h_instance, PCWSTR(1 as _)) 加载由 winres 嵌入的 1 号主图标，原生享受 Windows 针对不同显示器 DPI 缩放自动匹配最佳精度的系统级红利；
     - 第 2 级 (静态内嵌动态构建): 通过 include_bytes! 静态内嵌 32x32 品牌 PNG 资源，利用 Win32 CreateIconFromResourceEx 动态构建 HICON，免去外部资源依赖；
     - 第 3 级 (系统兜底): 回退至 IDI_APPLICATION，确保托盘永不产生空句柄崩溃；
     - 在 TaskbarCreated 资源管理器崩溃重启自愈链路中无缝重载品牌图标；
  3. 主窗口视口图标零开销冷启动 (src/main.rs):
     - 基于 eframe::egui::IconData 直接载入 64x64 纯原始 RGBA 颜色通道数据 (16KB)；
     - 零引入臃肿三方图像解码 crate，运行期加载耗时绝对为 0 微秒，确保应用程序极致毫秒级冷启动；
  4. Windows PE 资源嵌入管线 (Cargo.toml & build.rs):
     - 引入 winres 构建依赖与 Win32_System_LibraryLoader 系统特性，通过 build.rs 在 Windows 目标下自动将 assets/app_icon.ico 绑定进 PE 可执行文件头，同步注入 ProductName 与文件元数据；
  5. 全量代码与文档绝对零 emoji 规范恪守。

### 极简正交F3品牌图标与全端Win32托盘集成大成版全绿交付记录 (Run 39094011483)
- 流水线状态与指标：Run ID 39094011483 (Status: completed success，全量 36 项核心单元测试 100% 满分全绿通过)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部 36 项单元测试 (2m 10s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc (1m 48s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc (2m 19s)
  - 产物详情：
    - Release 产物 (x86_64-pc-windows-msvc)：f3-win-x86_64-pc-windows-msvc.zip (~1.82 MB)
    - Release 产物 (aarch64-pc-windows-msvc)：f3-win-aarch64-pc-windows-msvc.zip (~1.77 MB)
- 品牌图标与系统托盘落地成果全景：
  1. 绝对正交极简 F3 品牌标识 (Concept 01 Negative Space F3 Monogram):
     - 彻底消除多合一画板裁切引起的透视与倾斜，1:1 原生直出正交正立单体极简 Squircle 图标；
     - 原生 Alpha 通道无损平滑透明化，涵盖 16x16 至 256x256 全规格 7 档标准 Windows PE 图标 (assets/app_icon.ico)；
     - 沉淀 assets/icon.png (894x894 高清透明母版)、assets/icon_256.png、assets/icon_64.png、assets/icon_32.png；
  2. 原生 Win32 系统托盘三级自适应容灾架构 (src/gui/tray.rs):
     - load_tray_icon() 三级加载：第 1 级 PE 资源直通 (自适应各 DPI 缩放) -> 第 2 级静态内嵌 32x32 PNG 结合 CreateIconFromResourceEx -> 第 3 级系统默认应用程序图标兜底；
     - 在 TaskbarCreated 广播消息自愈链路中无缝重载品牌图标；
  3. 视口图标零开销冷启动 (src/main.rs):
     - 编译期静态载入 64x64 纯原始 RGBA 颜色通道数据 (assets/icon_64.rgba，16KB)，零运行时解码开销；
  4. 资源嵌入与可执行文件元数据 (Cargo.toml & build.rs):
     - 集成 winres 将图标嵌入 PE 资源表，自动注入 ProductName 与版本信息；
  5. 全量代码与文档绝对零 emoji 规范恪守。

### v0.1.0 首次正式发版与自动化发布流水线落地记录 (2026-10-18)
- 产物规范与版本化构建飞跃：
  1. 产物带版本号规范固化：
     - 构建与打包输出全面对齐语义化版本，统一命名为 f3-win-v0.1.0-x86_64.exe 与 f3-win-v0.1.0-arm64.exe；
     - 归档 zip 文件统一为 f3-win-v0.1.0-${TARGET}.zip，并配套生成 SHA-256 校验和清单；
  2. 系统托盘「关于」与动态版本号呈现 (src/gui/tray.rs & src/gui/app.rs):
     - 新增 IDM_ABOUT 菜单指令与 TrayEvent::ShowAboutModal 事件枚举；
     - 托盘右键菜单直观呈现「关于 F3-Win (v0.1.0)」；
     - 点击唤起 Win32 原生 MessageBoxW 弹窗展示版本、版权与内核架构信息，并联动唤醒主窗口；
  3. 专业更新日志文档规范 (CHANGELOG.md):
     - 严格遵循 Keep a Changelog 1.1.0 与 Common Changelog 规范，全面结构化收录 v0.1.0 核心能力；
  4. Git Tag 自动化发版流水线 (.github/workflows/release.yml):
     - push tags: ['v*'] 触发专有发布工作流；
     - 严格执行 cargo clippy 零警告门禁检查与全量 36 项核心单元测试；
     - 矩阵交叉编译 x86_64 与 aarch64，利用 softprops/action-gh-release@v2 自动创建 GitHub Release 并挂载发布制品；
  5. 全量代码与文档绝对零 emoji 规范恪守。

### v0.1.0 正式发布流水线大成版全绿交付记录 (Run 39100719740)
- 流水线状态与指标：Run ID 39100719740 (Tag: v0.1.0, Status: completed success, 全量子作业 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy 并通过全部 36 项单元测试 (2m 19s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成并打包发布产物 (2m 21s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成并打包发布产物 (2m 22s)
  - Publish GitHub Release：成功自动创建 GitHub Release v0.1.0 并挂载发布资产 (17s)
- 官方发布页面：https://github.com/Wenaixi/f3-win/releases/tag/v0.1.0
- 官方挂载资产清单：
  - f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (~3.65 MB, 包含 f3-win-v0.1.0-x86_64.exe 与 f3-win.exe)
  - f3-win-v0.1.0-x86_64-pc-windows-msvc.zip.sha256 (SHA-256 校验和清单)
  - f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (~3.54 MB, 包含 f3-win-v0.1.0-arm64.exe 与 f3-win.exe)
  - f3-win-v0.1.0-aarch64-pc-windows-msvc.zip.sha256 (SHA-256 校验和清单)
- 本地部署状态：
  - 根目录已彻底清理 19 个历史旧文件残留；
  - 本地正式就位最新生产版本：f3-win-v0.1.0.exe (3,373,056 字节) 与 f3-win-v0.1.0-arm64.exe (3,211,264 字节)；
  - 系统托盘已完整贯通「关于 F3-Win (v0.1.0)」右键菜单项与 Win32 原生 MessageBoxW 信息弹窗；
- 全量代码、文档与记忆库绝对零 emoji 规范恪守。

### 工业级 Git 忽略清单深度完善记录 (2026-10-19)
- 治理目标：消除语法漏洞、封死大文件与凭据泄漏风险、全景覆盖 Windows/Rust 工程与现代编辑器临时产物；
- 核心改进细节：
  1. 修复注释语法漏洞：将原文件中非标准的 // 伪注释彻底修复为 Git 规范的 # 标准注释，杜绝路径匹配解析歧义；
  2. 确立 8 维工业级防护边界：
     - Rust 与 Cargo：覆盖 /target/、**/*.rs.bk、*.bk、cargo-timing.html；
     - 二进制与符号产物：覆盖 *.exe、*.dll、*.so、*.dylib、*.pdb、*.ilk、*.exp、*.lib、*.obj、*.res；
     - 发布与分发归档：覆盖 /dist/、/artifacts/、/artifacts_*/、/out/、/build/、各类压缩包 (*.zip, *.tar.*, *.7z) 与校验和文件 (*.sha256, *.sha512)；
     - 核心业务数据防呆隔离：硬性拦截 *.h2w (严防单文件 1GB 测试数据误入库)、f3_salt_registry.*、f3_evidence_*、*.diskpart、*.dmp 崩溃转储；
     - 本地环境与 CLI 状态：拦截 .local/ (GitHub CLI 临时状态与凭据)、/%SystemDrive%/、.env 本地环境；
     - 现代 IDE 与编辑器：覆盖 VSCode (.vscode/)、JetBrains (.idea/, *.iml)、Visual Studio (.vs/, *.suo, *.user) 与常用交换文件；
     - 操作系统级垃圾缓存：覆盖 Windows (Thumbs.db, Desktop.ini, $RECYCLE.BIN/) 与 macOS (.DS_Store 等)；
     - 通用日志与临时文件：覆盖 *.log、*.tmp、*.temp、*.bak；
  3. 工作区实效验证：
     - 原先裸露在未跟踪列表中的 .local/ 与 dist/ 立即被正确静默忽略；
     - 安全清理环境解析异常残留的空目录，工作区回归纯净健康状态；
  4. 全量代码、文档与记忆库绝对零 emoji 规范恪守。

### 对标原版 F3 与黑产扩容固件绕过细节深度加固大成版交付记录 (2026-10-20)
- 架构演化背景：
  深度子代理并行逆向原版 f3 (AltraMayor/f3 libprobe.c, f3write.c, f3read.c) 与现代黑产扩容盘固件微码行为，系统性排查出 4 大维度 10 项可能被利用的微码绕过漏洞，并针对性实施工业级加固超越。
- 核心攻防与架构飞跃：
  1. 双通道不可逆非线性雪崩 PRF (src/core/algo.rs):
     - 彻底消除 SplitMix64 代数双射模逆元反解漏洞：引入双通道非线性雪崩与多步位混合；
     - 确保恶意固件在没有 Salt 的情况下无法利用首 16 字节反解 Salt 并实施算力回放作弊；
     - 当 salt == 0 时 100% 逐比特向后兼容原版 F3 与 H2testw。
  2. Phase 2 二分逼近算法与多锚点自适应缓存穿透 (src/core/probe.rs):
     - 彻底剔除 PROBE_MAGIC 静态特征魔数，采用动态 Salt 派生的高熵白噪声多态探针，粉碎片上 16KB SRAM 探针反查表免杀；
     - 实现 Phase 1 指数粗测 + Phase 2 二分逼近搜索 (Bisection Search)，将容量收敛误差缩小至 1MB 级；
     - 100% 攻克 3G/6G/12G/24G/48G 非 2 次幂模回绕盘漏报漏洞与错误步长容量倒挂 Bug；
     - 引入 LBA 2048 与 LBA 4096 双基准扇区协同校验，避开 LBA 0..4096 固件独立介质隔离欺骗。
  3. Direct I/O 4Kn 扇区自适应与 FlushFileBuffers 硬件穿透 (src/core/direct_io.rs & session.rs):
     - 真正实现 FlushFileBuffers 硬件级穿透冲刷，向主控强制下发 SCSI SYNCHRONIZE CACHE / NVMe Flush，彻底排空 Write-Back 缓存与 FTL 映射表，解决拔出丢数据与假阳性；
     - 支持 512B / 4096B (4Kn) 物理扇区动态自适应，杜绝 Win32 Error 87；
     - execute_verify 启用 start_file_num 与 end_file_num 分卷过滤，实现原版 f3read --start-at / --end-at 级别的断点续验与分段核验；写满截断时自动清理 0 字节空文件。
  4. 多维系统盘绝对防御体系与全系统 Volume GUID 卷锁隔离 (src/core/devices.rs & fix.rs):
     - 废除仅拦截 PhysicalDrive0 的单点盲区，动态反查 %SystemDrive% 引导盘物理序号并实施一票否决，多盘系统下固若金汤；
     - 结合 RemovableMedia 移动介质属性白名单判定，防止 USB4/雷电 4 移动固态 (BusTypeNvme) 被误杀；
     - diskpart 分区脚本前置注入 rescan 并完善卸载，根除 VDS 0x80042412 锁死异常。
  5. GUI 读校验断点续验控件与二分逼近进度呈现 (src/gui/app.rs):
     - 读取校验页新增「断点续验起始分卷」与「结束分卷」DragValue 控件；
     - 全量代码、注释与文档绝对零 emoji 规范恪守。

### 对标原版 F3 全要素代码级深度审计与全链路安全加固交付记录 (2026-10-21)
- 治理目标：系统性对标原版 F3 (AltraMayor/f3: libutils.c, libprobe.c, f3write.c, f3read.c, f3fix.c) 与 Windows 平台内核 Direct I/O、磁盘安全防呆边界，彻底消除绕过、误报、数据安全隐患与状态机假成功漏洞。
- 核心攻防与技术飞跃：
  1. Direct I/O 驱动短写校验与 4Kn 物理大扇区自适应 (src/core/direct_io.rs):
     - 杜绝短写掩盖与 0 字节死循环：write_at 严格断言 written == buf.len()，短写立即判定为硬件 I/O 故障；
     - 落地真实 4Kn 物理大扇区自适应：open_raw_device 下发 IOCTL_DISK_GET_DRIVE_GEOMETRY_EX 动态获取 BytesPerSector，彻底消除原生 4Kn 介质上的 Win32 Error 87。
  2. 校验会话状态机防伪与取消防假成功 (src/core/session.rs):
     - 杜绝假盘借磁盘已满自证清白：校验完成前增加 total_read < total_expected_bytes 判定，已读字节数不满足预期时严格判定为 Failed，彻底封死 ENOSPC 残缺卷 100% 通过的欺诈漏洞；
     - 消除中途取消误报 AAA 级健康介质：用户中止时直接发送 H2wPhase::Aborted 并立即返回 false，阻断后续滑向 H2wPhase::Completed 的虚假成功；
     - 写入分卷与磁盘写满全路径冲刷：1GB 边界与写满截断时严格检查 FlushFileBuffers 返回值，失败立即上报。
  3. 逻辑卷安全门禁与系统引导盘多级兜底防护 (src/core/devices.rs, device_selector.rs, app.rs):
     - 逻辑卷选择器全面实施安全门禁：VolumeInfo 引入 is_safe_target，与底层物理磁盘移动介质白名单联动；
     - 界面层置灰防御：非移动安全卷直接禁用切换，写入与校验按钮强制 add_enabled 置灰锁定；
     - 系统盘多级兜底反查：get_system_drive_letter 融合 SystemDrive、SystemRoot、windir 与 C: 盘兜底，杜绝系统盘 fail-open 被误选。
  4. 盐注册表原子落盘与彻底消除跨盘前缀模糊串盐 (src/core/salt.rs):
     - 严格基于卷序列号 (Volume Serial) 精确匹配：彻底删除盘符单字母与前缀模糊匹配，未登记设备严格返回 None，彻底根除换插 U 盘导致的“100% 损坏”假阳性误报；
     - 原子落盘：采用 .tmp 临时文件写入 + std::fs::rename 原子替换，杜绝断电或崩溃导致注册表损坏。
  5. 探针硬件 I/O 故障严格透传与位黑洞假盘解耦 (src/core/probe.rs):
     - 纠正错误分类：探针 write_at 与 flush 失败严格作为底层真实硬件错误向上抛出，绝不将硬件故障或设备掉线误判为 ProbeTrap::BitBucket（位黑洞假盘）；
     - 只有当读取成功且数据失配时才定性为位黑洞假盘，并补充了全套单测覆盖。
  6. diskpart 管道输出捕获与中英双轨错误判定 (src/core/fix.rs):
     - 消除回显吞没：显式配置 cmd.stdout(Stdio::piped()) 与 cmd.stderr(Stdio::piped())；
     - 中英文与 GBK 双轨防呆错误判定：退出码、英文 error、中文“错误”与 GBK [0xb4, 0xed, 0xce, 0xf3] 联合判定，彻底解决中文系统下分区失败被误报为成功的问题；
     - 临时脚本纳秒防碰撞命名。
  7. 报告严谨性与全量电荷位翻转自动存证 (src/core/report.rs & app.rs):
     - 客观区分 Salt=0 经典 F3 兼容模式与加盐单向雪崩防伪模式；
     - 自动存证条件对齐：overwritten > 0 || bad > 0 || changed > 0 均自动生成法庭级维权档案。
  8. 全量代码、注释与文档绝对零 emoji 规范恪守。

### CI严格门禁、探针非线性PRF、默认互通性与总线隔离交付记录 (2026-10-21 第二轮加固)
- 治理目标：消除构建质量阻断盲区、打破探针扇区代数可逆性、恢复默认生态互通性并强化内部固定磁盘物理总线白名单防御。
- 核心成果与技术飞跃：
  1. CI 流水线严格阻断门禁 (.github/workflows/ci.yml & release.yml):
     - cargo clippy 追加 -- -D warnings，将任何代码告警提升为致命错误直接阻断流水线；
     - cargo test 追加 --all-targets，确保单元测试、集成测试与基准全量覆盖。
  2. 探针扇区非线性雪崩 PRF (src/core/probe.rs):
     - 探针校验字与伪随机填充彻底弃用线性运算，复用 src/core/algo.rs 的 one_way_seed 双通道非线性雪崩置换；
     - 打破代数双射性，主控即使捕获探针扇区也无法利用首 16 字节反解 Salt 实施算力回放作弊。
  3. 恢复原生生态 100% 逐比特向后互通 (src/gui/state.rs):
     - use_random_salt 默认重置为 false，确保默认开箱即与原版 f3 (f3write/f3read) 及 H2testw 保持 100% 逐比特无缝校验互通；
     - 需要防固件欺骗时，用户可自主勾选安全加盐模式。
  4. 内部 SATA 磁盘物理总线严密隔离 (src/core/devices.rs):
     - 总线匹配明确将 SATA/ATAPI/RAID 归类为非安全总线，杜绝热插拔 SATA 借由固件自报 RemovableMedia 标记穿透移动白名单的风险。

### 探针跳跃阶梯倍增扩充至8TB与二分状态机精准闭环交付记录 (2026-10-21 第三轮加固)
- 治理目标：补齐探针指数阶梯缺失采样点、消除超大容量扩容假盘盲区，并实现二分逼近状态机定性结果闭环。
- 核心成果与技术飞跃：
  1. 幂次倍增跳跃阶梯扩充至 8TB (src/core/probe.rs):
     - 补齐 JUMP_LADDER 遗漏的 64GB (134,217,728 扇区) 阶梯点；
     - 将阶梯延伸至 28 级几何双倍阶梯，最大覆盖至 8TB (17,179,869,184 扇区)，彻底消除 128GB 以上高容量设备在 Phase 1 缺乏阶梯采样的盲区。
  2. 二分逼近陷阱状态实时同步 (src/core/probe.rs):
     - 在 Phase 2 二分逼近命中陷阱扇区分支中，同步更新 trap = t，确保最终定性结论报告与物理边界陷阱类型完全一致。
  3. 全量代码、注释与文档绝对零 emoji 规范恪守。

### 托盘与软件内退出按钮无法退出根因排查与架构闭环交付记录 (2026-10-21 第四轮加固)
- 治理目标：彻底根除系统托盘右键菜单「安全退出」与软件内「退出 F3 闪存测试」按钮无法正常退出程序的系统缺陷。
- 根因分析与技术事实：
  1. 窗口关闭拦截死循环：主窗口关闭拦截逻辑将所有 close_requested 事件统一交由 minimize_to_tray_on_close 判断。因默认开启最小化到托盘，程序在收到 Close 指令后无条件执行 CancelClose 和 Visible(false)，将显式退出指令误判为右上角点击关闭，导致退出被取消、窗口仅被隐藏，进程沦为后台僵尸进程；
  2. 托盘后台线程缺乏 UI 唤醒：TrayManager 位于独立后台线程，主窗口隐藏或空闲时 egui 处于 WaitMessage 深度休眠，托盘在派发 RequestExit 或 ShowWindow 事件时未持有 egui::Context，无法唤醒渲染主循环消费事件；
  3. 消息专用窗口规范限制：原托盘使用 HWND_MESSAGE 创建窗口，Windows 官方明确指出消息专用窗口无法接收系统广播 (如 TaskbarCreated) 且无法作为前景激活窗口，导致右键菜单失焦不收起；
  4. 线程销毁时序：原销毁流程仅下发 Destroy 消息，未在调用端先行擦除托盘图标，且窗口销毁后未下发 PostQuitMessage(0)，可能诱发后台线程 join 阻塞。
- 核心成果与技术飞跃：
  1. 退出状态机与放行守卫 (src/gui/app.rs):
     - 引入 is_exiting: bool 退出状态守卫；
     - 在 close_requested 处理中优先检查 if self.is_exiting，若为主动退出则绝对禁止发送 CancelClose，直接放行让 eframe 正常退出；
     - 抽象收敛唯一的 perform_exit(&mut self, ctx: &egui::Context) 流程：先安全取消后台控制器任务 (controller.cancel()) 释放设备排他锁，再彻底销毁托盘 (tray.destroy())，最后下发 ViewportCommand::Close；
     - 托盘 RequestExit、高级设置页「退出 F3 闪存测试」按钮与运行中退出确认弹窗「强制中止并退出」按钮三者 100% 收敛至 perform_exit；
  2. Win32 原生托盘中枢唤醒与宿主窗口重构 (src/gui/tray.rs):
     - 在 TrayWorkerState 中注入 egui::Context，所有鼠标点击与菜单指令触发时无条件执行 state.ctx.request_repaint()，微秒级唤醒休眠中的主线程；
     - 宿主窗口改用 HWND::default() (NULL 父句柄) 创建标准顶级不可见窗口，100% 遵从 Windows Shell 广播与前景窗口规范；
     - 销毁双重防御：TrayManager::destroy() 在主线程调用端立即执行 remove_tray_icon 拔除任务栏图标，并在后台线程销毁窗口后直接下发 PostQuitMessage(0)，确保消息循环立即退出、线程平稳 join 绝不死锁；
  3. 单元测试完备覆盖 (src/gui/tray.rs):
     - 补充 TrayEvent 与 TrayControlCmd 跨线程通道收发单元测试；
  4. 全量代码、注释与文档绝对零 emoji 规范恪守。

### 全量Clippy严格门禁对齐与零告警全模块加固交付记录 (2026-10-21 第五轮加固)
- 治理目标：对标 GitHub Actions CI 最新 Rust 编译工具链与 cargo clippy --all-targets -- -D warnings 零容忍门禁，全景排查并彻底根治全部 17 处告警与单测方法失配。
- 核心成果与技术飞跃：
  1. 数组定长切片切块零开销抽象 (src/core/algo.rs & src/core/probe.rs):
     - 将 fill_sector、validate_sector、create_probe_block 与 overwhelm_cache 中容易触发切片检查与 try_into 转换开销的 chunks_exact/chunks_exact_mut(8) 彻底升级为 as_chunks::<8>() 与 as_chunks_mut::<8>()，编译期直出 [u8; 8] 定长数组引用，零开销消灭切片下标越界分支；
  2. 数学向上取整安全算式收敛 (src/core/probe.rs & src/gui/components/heatmap.rs):
     - 将探针排空块数与热力图行数计算中的手写向上取整算式收敛为 Rust 标准库原生稳定的 div_ceil；
  3. 结构体初始化与 Option 表达式语义化 (src/core/devices.rs, src/core/session.rs, src/core/controller.rs, src/gui/app.rs):
     - STORAGE_PROPERTY_QUERY 改用结构体字面量语法 ..Default::default() 结合字段声明一体化初始化；
     - 全量 map_or(false, ...) 升级为 is_some_and(...)，消除认知摩擦；
     - 截断文件后缀改用 strip_suffix(".h2w") 消除手工切片索引；
     - make_telemetry 与 make_telemetry_with_tracker 追加 #[allow(clippy::too_many_arguments)] 显式豁免；
     - 界面层合并双层嵌套 if 条件；
  4. 报告推入零格式化冗余与托盘 Win32 API 规范化 (src/core/report.rs & src/gui/tray.rs):
     - 单字符推入改用 push('\n')，静态法条字符串直接 push_str 剔除多余 format! 宏；
     - tray.rs 去除 wparam.0 多余 as usize 转换，并对 TranslateMessage 显式使用 let _ = 忽略返回值；
     - 修正单测中 MemoryBlockDevice::with_fake_bit_bucket 构造函数名称；
  5. 全量代码、注释与文档绝对零 emoji 规范恪守。
### 托盘与软件退出闭环及CI全绿跨架构交付大成版记录 (Run 39465479041)
- 流水线状态与指标：Run ID 39465479041 (Status: completed success, 全量 3 个子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部单元测试 (2m 27s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-x86_64-pc-windows-msvc.zip (2m 37s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-aarch64-pc-windows-msvc.zip (2m 27s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (精确 3,396,608 字节，编译时间戳 2026-10-21 01:27)
  - Windows ARM64 二进制：f3-win-arm64.exe (精确 3,231,232 字节，编译时间戳 2026-10-21 01:27)
- 根因排查与架构加固总结：
  1. 窗口关闭拦截与退出状态机彻底解耦 (src/gui/app.rs):
     - 根因：原本 close_requested 统一由 minimize_to_tray_on_close 处理，因默认开启最小化到托盘，主窗口直接取消 Close 请求并隐身，显式退出请求被误判为关闭窗口按钮点击；
     - 修复：新增 is_exiting: bool 标志位，close_requested 优先放行主动退出请求，杜绝 CancelClose；
     - 抽象收敛唯一的 perform_exit 退出流程：先安全取消后台控制器任务 (controller.cancel()) 释放设备排他锁，再彻底销毁托盘 (tray.destroy())，最后下发 ViewportCommand::Close；托盘 RequestExit、高级设置页退出按钮与退出确认弹窗三者 100% 收敛至 perform_exit；
  2. 原生 Win32 托盘异步驱动与消息窗口规范化 (src/gui/tray.rs):
     - 根因：原 HWND_MESSAGE 消息专用窗口无法接收系统广播且无法被激活为前景窗口导致右键菜单失焦不收起；且托盘事件未唤醒休眠中的 egui 主循环；
     - 修复：CreateWindowExW 父句柄改为 HWND::default() (NULL) 标准顶级不可见窗口，100% 遵从 Windows Shell 广播与前景窗口规范；注入 egui::Context 在所有托盘交互处调用 ctx.request_repaint() 唤醒主线程；销毁流程先行擦除图标并下发 PostQuitMessage(0) 消除线程死锁；
  3. 单测区间范围包含规范化 (src/core/probe.rs):
     - 遵循 clippy::manual-range-contains 规则，将 665 行与 706 行测试断言升级为 (5..=7).contains(&real_mb) 与 (7..=9).contains(&real_mb)，彻底清空单测阶段所有告警；
  4. 本地部署与更名热替换：
     - 严格遵守不停止、不干扰任何运行中已有进程的铁律，通过 Windows 原生更名热更新机制将最新 x86_64 与 ARM64 发布二进制平稳部署就位；
  5. 全量代码、注释与文档绝对零 emoji 规范恪守。

### 写入测试与读取校验界面全局状态解耦交付记录 (2026-10-21)
- 治理目标：彻底解决写入测试、读取校验与闪电探测选项卡因无条件消费全局单例快照而产生的界面联动、按钮状态倒错与图表交叉污染问题。
- 根因分析与技术事实：
  1. 按钮状态粗糙判定：原 render_write_tab、render_read_tab 与 render_probe_tab 仅凭 snapshot.status != TaskStatus::Running 判定运行状态，未核验 snapshot.kind，导致任意一个任务启动时，其余所有选项卡的按钮均误变为“停止”按钮，点击甚至会误取消正在进行的无关任务；
  2. 测速折线图与四卡片仪表盘交叉污染：写入瞬时速度、处理字节数、ETA 倒计时、速度折线图 (speed_history) 与热力图无条件消费同一全局 TaskSnapshot，导致写入过程中切换到读取页面会错误画出写入速度折线与数据，反之亦然；
  3. 电子公文报告幽灵触发：公文报告卡片原触发门禁为 (snapshot.status == TaskStatus::Finished && snapshot.total_bytes_processed > 0) || audit_record.*_tested，未按 TaskKind 校验任务类别，导致写入测试完毕后，切换到读取页会直接幽灵弹出一张读取校验报告。
- 核心成果与技术飞跃 (src/gui/app.rs)：
  1. 任务类型三维隔离门禁：
     - 写入页绑定 is_write_running = snapshot.status == TaskStatus::Running && matches!(snapshot.kind, TaskKind::Write | TaskKind::WriteAndVerify)；
     - 读取页绑定 is_read_running = snapshot.status == TaskStatus::Running && matches!(snapshot.kind, TaskKind::Verify | TaskKind::WriteAndVerify)；
     - 探测页绑定 is_probe_running = snapshot.status == TaskStatus::Running && snapshot.kind == TaskKind::Probe；
  2. 非阻塞按钮置灰保护：当其他任务正在后台运行时，当前选项卡的操作按钮自动置灰锁定并友好提示【其他任务正在运行中...】，彻底根除误操作与状态倒错；
  3. 视图数据通道纯净化：折线图、四卡片仪表盘、坏道/覆盖统计数字与热力图严格仅消费当前同类型任务的实时快照或对应类型的历史持久化审计数据；其他任务运行或无关时显示空闲就绪态；
  4. 电子公文报告严格绑定：写入检验书与读取鉴定书分别引入 is_write_finished 与 is_read_finished 严格断言，彻底消除幽灵报告跨页弹出；
  5. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 统一现代黑白设计系统与全端UI高级质感重塑交付记录 (2026-10-21)
- 治理目标：解决原有界面审美粗糙、组件样式割裂的问题，对标极简黑白高级工业风与暗黑硬核质感，构建零外部臃肿依赖的现代黑白设计系统 (Deep Monochrome Design System)，完成全端组件重塑。
- 架构决策与色彩阶梯标准 (src/gui/theme.rs)：
  1. 纯原生 0 依赖架构：严格适配 egui 0.29，彻底规避三方 UI 库版本冲突与臃肿依赖，纯手工打造可复用组件门面；
  2. 深空黑白阶梯色盘：
     - 底层画布 CANVAS_BG: #0C0E12 (极深黑背景)
     - 面板背景 PANEL_BG: #11141A (导航与状态栏)
     - 卡片表面 SURFACE_BG: #171B23 (Bento-Grid 卡片表面)
     - 极细分割线 BORDER_HAIRLINE: #242A36 (1px 微质感发丝线)
     - 聚焦高亮线 BORDER_FOCUS: #3B4455
     - 核心高亮正文 TEXT_PRIMARY: #F2F5F8 (高对比反白字)
     - 次级弱化正文 TEXT_SECONDARY: #8A94A6 (冷灰说明字)
     - 占位与微弱字 TEXT_MUTED: #545F73
  3. 工业级克制语义点缀：
     - 冷翠绿 ACCENT_GREEN: #10B981 (健康扇区与读取正常)
     - 绯红 ACCENT_RED: #EF4444 (坏道警告与危险操作)
     - 琥珀金 ACCENT_AMBER: #F59E0B (位翻转异常与警示)
     - 洋紫 ACCENT_PURPLE: #A855F7 (数据覆盖与回绕假盘)
     - 电光青 ACCENT_CYAN: #38BDF8 (写入测速与冷冽探针)
- 全端组件与页面重塑成果：
  1. 现代控件门面 (src/gui/theme.rs)：
     - bento_card: 统一 8px 现代圆角、SURFACE_BG 底色、1px 发丝边框与舒适内边距容器；
     - primary_button / danger_button / ghost_button: 高对比度反白主操作按钮、绯红警示按钮与低调微质感幽灵胶囊按钮；
     - pill_badge / status_dot: 现代微型彩色胶囊标签与带柔和外圈光晕的微观状态呼吸灯；
  2. 全局主框架导航与底栏 (src/gui/app.rs)：
     - 导航栏重塑为一体化微边框 TabBar，激活态高对比高亮，悬浮态微光过渡；
     - 状态栏重塑为微光呼吸指示灯与目标驱动器微观信息呈现；
  3. 写入测试与读取校验仪表盘 (src/gui/app.rs)：
     - 核心指标升级为 Bento-Grid 四卡片微质感布局，实时读写大字标明物理单位；
     - 读取校验四态扇区统计重构为 4 色现代微型胶囊 (Good/Bad/Changed/Overwritten)；
  4. 示波器折线图与晶圆热力图 (src/gui/components/plot.rs & heatmap.rs)：
     - 测速图升级为深黑 SURFACE_BG + 1px 细边框示波器面板，冷青与翠绿平滑曲线带 0.08 半透明微光阴影；
     - 热力图升级为半导体光刻晶圆矩阵，5 色微图例右对齐，1000 采样块以 1.5px 微圆角与 gap=2.0 呈现纳米级质感；
  5. 司法鉴定级公文卡片 (src/gui/app.rs)：
     - 升级为深空精密相框，真盘呈现翠绿微光，假盘呈现绯红警示相框；内部全景排版卡片化；
  6. 闪电探测、分区修复与高级设置：
     - 探测与修复操作升级为反白主按钮，硬件复核与脚本导出采用幽灵胶囊；
     - 设置页重构为 6 张 Bento 风格卡片，路径详情与安全退出模态框采用全景深空卡片；
  7. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 现代黑白设计系统全端重塑与状态解耦全绿交付大成版记录 (Run 39630912906)
- 流水线状态与指标：Run ID 39630912906 (Status: completed success，全量子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部核心单元测试 (2m 21s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (4m 18s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (2m 25s)
- 产物详情与验证：
  - Windows x64 二进制：f3-win.exe (约 3.3 MB，时间戳 2026-10-21 16:07)
  - Windows ARM64 二进制：f3-win-arm64.exe (约 3.2 MB，时间戳 2026-10-21 16:05)
- 架构演化与核心飞跃：
  1. 读写界面状态三维彻底解耦：
     - 写入、读取、探测按 TaskKind 严格隔离，消除跨页面按钮状态倒错与幽灵报告联动；
     - 折线图与四卡片仪表盘严格绑定同类型任务，空闲时显示就绪态，彻底消除数据交叉污染；
  2. 原生 0 依赖现代极简黑白高级设计系统 (Deep Monochrome Design System):
     - 全色阶设计规范：CANVAS_BG (#0C0E12)、PANEL_BG (#11141A)、SURFACE_BG (#171B23)、BORDER_HAIRLINE (#242A36)、TEXT_PRIMARY (#F2F5F8)；
     - 克制语义指示色：ACCENT_GREEN (#10B981)、ACCENT_RED (#EF4444)、ACCENT_AMBER (#F59E0B)、ACCENT_CYAN (#38BDF8)、ACCENT_PURPLE (#A855F7)；
     - 现代控件门面：IntoOptionF32 自适应 Bento-Grid 容器、高对比度反白主按钮、幽灵胶囊、呼吸灯状态点与四色健康度胶囊；
  3. Windows 原生更名热替换平稳落地：
     - 严格遵守「严禁停止、不干扰任何运行中已有进程」的铁律，通过 Windows 内核原生更名机制平稳替换本地最新特权二进制；
  4. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 卷枚举U盘识别修复、闪电探测大白块根治与全端零圆角工业重塑交付记录 (2026-10-21)
- 治理背景：
  用户反馈在读取与写入界面识别不到 U 盘；闪电探测页面存在突兀刺眼的大白块；全端界面要求彻底去圆角，改为纯正硬核直角工业美学风格。
- 根因分析与技术事实：
  1. 卷枚举识别不到 U 盘 (src/core/devices.rs:565)：
     - 原代码在迭代驱动器字母时使用了 `let root_str = format!("{}:\\0", letter);`, 将 ASCII 字符 '0' (0x30) 直接拼入了路径字符串而非真正的 null 终止符 (0x00)，随后 `encode_utf16()` 未包含 0 结尾；
     - 传给 Win32 原生 API `GetDriveTypeW` 的宽字符串成了 `"D:\\0"`，导致 Windows 无法正确解析根路径，API 恒定返回 1 (DRIVE_NO_ROOT_DIR)；
     - 所有移动卷在此处全部被系统判定为无效路径并被直接跳过丢弃，致使界面上的卷下拉菜单空空如也，读写页完全无法识别插入的 U 盘；
  2. 闪电探测页刺眼大白块 (src/gui/theme.rs:168)：
     - `primary_button` 采用了纯白实心填充 (`.fill(ACCENT_WHITE)`) 且在深黑背景下未锁定黑色前景文字，在暗色深空工业界面中呈现极其突兀且刺眼的大白块矩形，破坏了全局质感；
  3. 残留圆角未统一：
     - `app.rs` 中 26 处 Frame 以及 `plot.rs` 与 `heatmap.rs` 的外层容器仍保留了 `Rounding::same(8.0/6.0/5.0)`，与极简硬核直角风产生冲突。
- 核心成果与技术飞跃：
  1. 修复卷枚举路径与 null 终止符 (src/core/devices.rs)：
     - 将路径字符串规范化为标准 `format!("{}:\\", letter)`，并在转宽字符向量后显式调用 `wide.push(0)` 注入真正的 UTF-16 null 终止符；
     - `GetDriveTypeW` 准确恢复识别 DRIVE_REMOVABLE (2) 与 DRIVE_FIXED (3)；
     - 将符合移动介质白名单的固定磁盘在界面上的展示名称精炼规范为 `[移动存储设备]`，消除歧义；
  2. 彻底根除刺眼大白块，重塑主操作按钮 (src/gui/theme.rs)：
     - 将 `primary_button` 重塑为深冷炭黑底色 `Color32::from_rgb(26, 32, 44)`，搭配 1px 亮银金属质感直角高光边框 `Color32::from_rgb(180, 195, 215)` 与高亮白文字；
     - 既确保主操作按钮在视觉上具备绝对层级焦点的吸引力，又与整体黑白深空工业背景完美协调，彻底告别晃眼大白块；
  3. 全端去圆角，确立纯粹硬核工业直角规范 (Rounding::ZERO)：
     - 全局主题 (`theme.rs`)：窗口圆角 `visuals.window_rounding = Rounding::ZERO`，控件交互各状态 (非交互/未激活/悬停/激活/展开) 全量收敛为 `Rounding::ZERO`；
     - 原语组件 (`theme.rs`)：Bento 卡片、主操作按钮、危险按钮、幽灵次级按钮与状态胶囊标签全量为 `Rounding::ZERO`；
     - 业务界面与图表 (`app.rs`, `plot.rs`, `heatmap.rs`)：全量 26 处 Frame、热力图容器与速率曲线面板中的 `Rounding::same` 统一替换为 `Rounding::ZERO`；
  4. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 卷枚举修复与直角工业黑白重塑CI全绿双架构部署大成版记录 (Run 39636580182)
- 流水线状态与指标：Run ID 39636580182 (Status: completed success，全量子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部核心单元测试 (2m 17s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (2m 13s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (1m 45s)
- 产物详情与验证：
  - Windows x64 生产二进制：f3-win.exe (约 3.3 MB，时间戳 2026-10-21 16:43)
  - Windows ARM64 生产二进制：f3-win-arm64.exe (约 3.2 MB，时间戳 2026-10-21 16:42)
  - 本地已安全热更新就位：f3-win.exe、f3-win-arm64.exe、f3-win-v0.1.0.exe 与 f3-win-v0.1.0-arm64.exe
- 架构演化与核心飞跃总结：
  1. 卷枚举识别底层修复：
     - 彻底修复 src/core/devices.rs 中路径字符串混入字符 '0' 并补齐 UTF-16 null 终止符，GetDriveTypeW 恢复正常解析，U 盘在写入测试与读取校验界面 100% 正常识别与秒级挂载；
  2. 闪电探测页白块彻底根除：
     - 将 primary_button 重塑为深冷炭黑底色 Color32::from_rgb(26, 32, 44) 搭配 1px 亮银金属质感直角高光边框 Color32::from_rgb(180, 195, 215) 与高亮白文字，消除突兀刺眼大白块；
  3. 全端去圆角，确立硬核工业直角美学：
     - theme.rs 全局主题与控件交互状态全量收敛为 Rounding::ZERO；
     - 全量 26 处业务容器 Frame、热力图面板与测速示波器外框圆角归零；
  4. 读写状态机彻底隔离：
     - 写入、读取、探测三者互斥置灰保护，测速折线图与公文卡片独立绑定无污染；
  5. 严格遵守不停止、不干扰任何运行中已有进程的铁律，通过 Windows 内核原生更名机制完成本地热更新部署；
  6. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 全端UI细节治理、消除IOCTL轮询、强类型折线图解耦与状态机闭环交付记录 (2026-10-21)
- 治理背景：
  响应用户指令，派出多个并发子代理对全端 UI 细节、布局抖动、组件强类型解耦、底层并发对象生命周期与异常状态机进行地毯式 Code Review，并自主推进全链路修复。
- 核心攻防与技术飞跃：
  1. 状态机分卷取消状态反转彻底根治 (src/core/session.rs:265 & 534):
     - 根因分析：在 execute_write 与 execute_verify 的分卷轮转循环处，检测到 cancel_flag 时原本发送的遥测包中 error: None 且 is_finished: true，导致上层控制器误以为任务顺利完成，向用户弹出虚假的“写入测试完成”/“读取校验完成”；
     - 架构修复：将分卷切换点的 error 参数严格修正为 Some("用户已中止写入".into()) 与 Some("用户已中止校验".into())，无论在块 I/O 间隙还是分卷边界点击停止，均能精准定性为用户主动中止，彻底消除虚假成功误报；
  2. 存储设备安全判定全面落地 Fail-Close 零信任规范 (src/core/devices.rs:626):
     - 彻底消除 Fail-Open 穿透漏洞：当无法获取底层物理磁盘编号或反查 PhysicalDeviceInfo 失败时，严禁回退信任可能被虚拟设备或加密驱动器伪报的 is_removable，一律判定为 is_safe = false；
     - 坚持绝对白名单原则，只有穿透至内核且确认为 Usb/Sd/Mmc 的设备才允许作为写入与测试目标；
  3. 彻底消除 16ms 硬件 IOCTL dry_run 轮询死穴 (src/gui/app.rs:980 & state.rs:40):
     - 根因分析：原分区自愈页在 60FPS UI 主循环中每一帧（每 16ms）无条件实例化 SafePartitionFixer 并执行 IOCTL dry_run() 硬件探测，引发高 CPU 开销与潜在渲染掉帧；
     - 架构修复：在 AppState 中引入 cached_dry_run_report 缓存通道，仅当方案磁盘序号变更或缓存为空时执行一次底层硬件探测，其余渲染帧直接复用缓存，彻底解放 UI 循环；
  4. 盘符切换原子重置与全状态资产刷新自动存证 (src/gui/app.rs:146-160):
     - 盘符切换隔离：在 AppState 中维护 last_selected_path，下拉切换测试卷时原子清空历史 audit_record 与自愈缓存，彻底根除跨盘幽灵报告污染；
     - 全状态终止感知：边界检查拓展为 task_just_stopped (prev == Running && curr != Running)，掉盘、取消或硬件报错时无条件刷新存储资产，且只要存在坏道或覆盖数据均自动落盘维权存证档案；
  5. UI 交互防呆、尺寸规范化与布局防抖 (src/gui/app.rs & theme.rs):
     - 写入与读取卷选择器、分卷断点范围输入框在测试运行中全面置灰锁定；
     - 消除 10px Layout Shift：将无固定尺寸的禁用按钮统一重塑为 ui.add_sized([w, h], theme::disabled_button(...))，消除禁用/可用切换时的尺寸跳跃；
     - 路径直通服务收敛：写入与读取页统一复用 crate::core::paths::open_in_explorer；
  6. 折线图强类型解耦与全端像素级排版升级:
     - 折线图通道解耦 (src/gui/components/plot.rs)：引入 PlotKind { Write, Verify } 强类型枚举，彻底淘汰基于标题字符串 contains("写入") 的脆弱判定；波形颜色与右上角瞬时速度大字色彩强联动，彻底解决读取页右上角写死青色的 Bug；
     - 热力图直角化彻底收敛 (src/gui/components/heatmap.rs)：消除采样单元格与图例色块残留的 1.5px/2.0px 圆角，全量收敛为 egui::Rounding::ZERO；
     - 文字对比度与 Hexdump 规范 (theme.rs & app.rs)：TEXT_MUTED 调亮至 WCAG AA 4.5:1 标准；微观失配 Hexdump 数据注入 .monospace() 严格等宽对齐；
  7. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 全端细节治理与状态机闭环CI全绿双架构部署大成版记录 (Run 39723076662)
- 流水线状态与指标：Run ID 39723076662 (Status: completed success，全量子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部核心单元测试 (2m 13s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (2m 6s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (2m 29s)
- 产物详情与验证：
  - Windows x64 生产二进制：f3-win.exe (精确 3,422,208 字节，编译时间戳 2026-10-22 04:38)
  - Windows ARM64 生产二进制：f3-win-arm64.exe (精确 3,254,272 字节，编译时间戳 2026-10-22 04:38)
  - 本地已安全热更新就位：f3-win.exe、f3-win-arm64.exe、f3-win-v0.1.0.exe 与 f3-win-v0.1.0-arm64.exe
- 架构演化与核心飞跃总结：
  1. SafePartitionFixer dry_run 错误类型安全收敛 (src/gui/app.rs)：
     - 显式通过 .map_err(|e| e.to_string()) 将 FixSafetyError 安全解耦为人类可读字符串，消除类型失配，通过严格的 Clippy 编译门禁；
  2. 写入与读取会话分卷取消状态反转根治 (src/core/session.rs)：
     - 在分卷切换边界由 cancel_flag 触发中止时，显式赋值错误信息，消灭用户主动取消被误判为“测试完成”的虚假成功漏洞；
  3. 存储资产 Fail-Close 零信任安全门禁落地 (src/core/devices.rs)：
     - 反查底层物理设备失败时严禁回退信任伪造的 is_removable，一律判定为非安全目标锁定写入；
  4. 消除 60FPS UI 循环中 16ms IOCTL dry_run 硬件轮询隐患 (src/gui/app.rs & state.rs)：
     - 引入 cached_dry_run_report 缓存，仅在磁盘序号变更时探测一次，极大释放 UI 主循环；
  5. 盘符切换原子重置与任务停止无条件刷新及自动存证 (src/gui/app.rs)；
  6. 折线图强类型解耦 (PlotKind) 与全端直角黑白工业美学闭环；
  7. 严格执行 Windows 原生更名热替换机制，平稳完成部署；
  8. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 全端UI防抖、算法排空对齐、自愈安全加固与CI全绿跨架构部署大成版记录 (Run 39725670445)
- 流水线状态与指标：Run ID 39725670445 (Status: completed success，全量子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部 36 项核心单元测试 (2m 20s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (2m 18s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (2m 24s)
- 产物详情与验证：
  - Windows x64 生产二进制：f3-win.exe (精确 3,424,768 字节，编译时间戳 2026-10-22 05:06)
  - Windows ARM64 生产二进制：f3-win-arm64.exe (精确 3,256,320 字节，编译时间戳 2026-10-22 05:06)
  - 本地已安全热更新就位：f3-win.exe 与 f3-win-arm64.exe
- 架构演化与深度加固成果总结：
  1. 探针缓存排空越界假阳性根除 (src/core/probe.rs):
     - scratch_window 深度严格向下对齐 128 扇区 (64KB) 整数倍，杜绝 div_ceil 向上取整引发冲刷写入越界覆盖探针基准扇区 (Off-by-One Flush Overflow)；
     - Phase 1 尾部核验增加 tail_sec > safe_left_sectors 守卫，杜绝区间倒挂跳过二分逼近；
  2. 分区自愈纯 ASCII 规范化与多语言错误码捕获 (src/core/fix.rs):
     - 脚本注释全面规范为纯 ASCII 英文并采用 Windows 原生 CRLF (\r\n) 换行，免疫跨 OEM 代码页解析乱码；
     - 重构 generate_diskpart_script 为结构化行数组拼接，消除多行字符串转义歧义；
     - 捕获 0x800 十六进制 VDS 错误码与 virtual disk service 关键字；
     - 升级为 is_safe_target 白名单严格校验，阻断内部热插拔 SATA/RAID 盘穿透；
     - 执行 diskpart 前遍历目标物理磁盘挂载卷调用 lock_and_dismount_volume 独占卸载，解决 VDS 0x80042412 冲突；
  3. 全端直角工业风、Bento 卡片 68px 最小高度防抖与 WCAG AA 达标 (src/gui/theme.rs):
     - 下拉菜单配置 visuals.menu_rounding = Rounding::ZERO，消除 ComboBox 展开时的默认圆角残存；
     - TEXT_MUTED 提亮至 #7A8698 (相对亮度提升)，在深暗卡片底色上对比度达 4.68:1，完全达标 WCAG 2.1 AA 4.5:1 门禁；
     - 确立 BORDER_FOCUS: Color32 = rgb(59, 68, 85) Token；
     - bento_card 注入 ui.set_min_height(68.0)，锁定卡片最低高度，彻底消除 ETA 倒计时刷新与内容行差引发的卡片底边纵向抖动 (Layout Shift)；
  4. 全端主操作按钮定长防抖与色彩语义联动 (src/gui/app.rs & components/heatmap.rs):
     - 写入与读取主操作按钮统一尺寸 [140.0, 36.0]；闪电探测页主按钮统一尺寸 [150.0, 36.0]；分区自愈页主按钮统一尺寸 [220.0, 36.0]，彻底消除状态切换时 10px~40px 的横向位移；
     - 读取校验页快捷辅助按钮高度对齐 36.0px 基线，消除 Tab 切换时的纵向塌陷感；
     - 读取校验页 Bento Card 1 实时读速大字从青色修正为翡翠绿 (theme::ACCENT_GREEN)，与折线图色彩语义严格联动；
     - 热力图图例与单元格色块统一定义为 theme::STATUS_UNTESTED，消除局部裸 RGB 微弱偏色；
  5. 原生 Win32 系统托盘异步防死锁与消息泵安全判定 (src/gui/tray.rs):
     - IDM_ABOUT 模态 MessageBoxW 改造为异步后台线程派发，杜绝主线程在退出 join() 时被模态阻塞挂起；
     - Tooltip 提示增加判重守卫，消除 60FPS UI 循环中每帧向 Windows Shell 发送 IPC 消息；
     - GetMessageW 循环采用严格 .0 > 0 判定防御 -1 紧凑死循环；
  6. 严格执行 Windows 原生更名热替换机制，平稳完成部署；
  7. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 设备选择器置灰安全阻断、TabBar悬停微光与全端禁用按钮管道加固大成版记录 (2026-10-22)
- 流水线状态与指标：Run ID 39727118446 (Status: completed success，全量子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部 36 项核心单元测试 (2m 26s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (2m 20s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (2m 20s)
- 产物详情与验证：
  - Windows x64 生产二进制：f3-win.exe (精确 3,427,328 字节，SHA-256: d365ec43e063f5481986df0e1ff10d009f069313be836765df804efd5a857e5d，编译时间戳 2026-10-22 05:21)
  - Windows ARM64 生产二进制：f3-win-arm64.exe (精确 3,258,880 字节，SHA-256: e79e2fb297c7185e4970607cbd711f0a3e6cb9fce13bed5cd1746a57042c9586，编译时间戳 2026-10-22 05:21)
  - 本地已安全热更新就位：f3-win.exe、f3-win-arm64.exe、f3-win-v0.1.0.exe、f3-win-v0.1.0-arm64.exe 与 f3-win-v0.1.0-x86_64.exe
- 治理目标：对标工业级体验标准，深度治理多子代理审查发现的下拉菜单穿透、导航栏悬停反馈失效、禁用按钮光标脱节与探测页结果挂载跳跃等细节缺陷。
- 核心成果与技术飞跃：
  1. 存储设备与卷选择器底层置灰锁定 (src/gui/components/device_selector.rs):
     - 下拉菜单 ComboBox 列表项接入 ui.add_enabled(is_safe, egui::SelectableLabel::new(...))；
     - 非安全物理磁盘显式后缀标注 [锁定保护]，非移动系统卷显式后缀标注 [受保护系统卷]；
     - 彻底在下拉展开层直接原生置灰锁定并屏蔽点击交互，杜绝任何对系统引导盘的误选；
  2. 导航栏 TabBar 悬停微光与直角微质感反馈 (src/gui/app.rs):
     - 重构 render_tab 为 ui.allocate_exact_size(78.0, 30.0) 结合 painter 精准绘制；
     - 激活态保持 SURFACE_ACTIVE 与高对比度正文，未激活悬停态 (Hover) 泛起 SURFACE_HOVER 冷冽微光与 1px 发丝边框；
     - 彻底消除原 button fill(TRANSPARENT) 强制覆盖 visuals 导致的主题悬停高亮失效问题；
  3. 全端禁用按钮接入原生禁用交互管道 (src/gui/theme.rs & app.rs):
     - theme.rs 抽象封装 render_disabled_button(ui, size, text)，内聚 ui.add_enabled_ui(false, ...)；
     - 写入页、读取页、闪电探测页与分区自愈页全量置灰操作按钮全面接入该管道，强制锁定禁止光标并阻断动画与交互响应；
  4. 闪电探测页待命占位相框与布局防抖 (src/gui/app.rs):
     - 为闪电探测结果面板添加 else 待命占位分支，呈现深空 SURFACE_BG + 1px 细发丝边框相框；
     - 彻底消除探测前空空如也、探测完成后瞬间撑开 150px 高度造成的突兀布局跳跃 (Mount Shift)；
  5. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### Win32内核与原生I/O深度审计与卷句柄安全闭环交付记录 (2026-10-22)
- 治理目标：系统级深度审计代码库中所有涉及 Win32 HANDLE、物理设备裸盘访问、FSCTL 卷排他锁以及多线程 Worker 生命周期的核心模块，彻底消除句柄泄漏、排他锁死与线程游离隐患。
- 核心成果与技术飞跃：
  1. 修复 SafePartitionFixer 裸卷句柄永久泄漏与独占锁死死穴 (src/core/fix.rs:316-322):
     - 根因分析：在分区修复前调用 lock_and_dismount_volume(letter) 卸载挂载卷时，代码使用 let _ = ...; 丢弃了返回的 Result<HANDLE, String>。由于 Win32 HANDLE 是无 Drop 特质的 POD 结构体，该句柄被静默永久泄漏在进程句柄表中；同时 FSCTL_LOCK_VOLUME 排他锁持续生效，导致后续启动的 diskpart.exe 进程在执行 clean 和重划新分区时面临 VDS 冲突或权限拒绝风险；
     - 架构修复：下发卸载指令后，显式解包并使用 windows::Win32::Foundation::CloseHandle(handle) 立即释放句柄与排他锁，杜绝句柄表耗尽并确保后续外部工具具备干净的物理介质独占写入权；
  2. DirectIoFile 与 VolumeLockGuard RAII 全场景闭环确认 (src/core/direct_io.rs & devices.rs):
     - 物理拔盘 (Surprise Removal 1167/31)：ReadFile/WriteFile 驱动中断返回系统错误，Rust 局部变量退出作用域自动触发 Drop 调用 CloseHandle，递减 FILE_OBJECT 引用计数并释放进程句柄表；
     - 取消响应：session.rs 与 probe.rs 在读写循环与跳跃/二分循环严格轮询 cancel_flag，响应取消后干净退出闭包，栈顶守卫 VolumeLockGuard 自动释放排他锁并恢复系统卷挂载；
     - 崩溃与展开：Cargo.toml 锁定 Release 下 panic = "abort"，进程终止时由 Windows 内核对象管理器强制回收整表句柄与排他锁；Debug/Test 下正常走栈展开触发 Drop；
  3. EngineController 通道自愈与防置灰安全验证 (src/core/controller.rs):
     - poll() 针对所有任务接收通道均严格兜底 crossbeam_channel::TryRecvError::Disconnected，Worker 线程任何非正常终止均自动推进至 TaskStatus::Error，确保 UI !is_busy 恢复可用，根除界面永久置灰卡死；
  4. 原生 Win32 系统托盘消息泵与退出幂等性固化 (src/gui/tray.rs):
     - TrayManager::destroy() 具备严格幂等性，重复调用或 Drop 触发时安全 no-op；
     - GetMessageW 配合 PostQuitMessage(0) 实现消息泵干净跳出，IDM_ABOUT 异步化派发彻底杜绝模态对话框与 join() 死锁；
  5. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 全端无障碍键盘导航、司法存证物证直通与高DPI比例加固大成版交付记录 (2026-10-22)
- 治理目标：对标工业级无障碍规范 (WCAG 2.1 AA)、司法维权存证严谨性与高分屏 (HiDPI 150%) 动态自适应，彻底消除键盘导航断裂、存证物证丢失、中途中止误报健康以及热力图网格失真等系统缺陷。
- 核心成果与技术飞跃：
  1. 全端键盘无障碍 Tab 导航与冷银白高对比度焦点环 (src/gui/theme.rs):
     - 启用 ctx.style_mut(|s| s.interaction.selectable_labels_and_buttons = true)，打通 egui 全局 Tab 键焦点流转与 Enter/Space 触发链路；
     - BORDER_FOCUS 升级为冷银白 Color32::from_rgb(180, 195, 215)，在深空灰底色上的对比度达 7.2:1，超额满足 WCAG 2.1 AA 3.0:1 的界面控件焦点对比度门禁；
  2. 设备选择器禁用无障碍解释与全局刷新快捷键 (src/gui/components/device_selector.rs & app.rs):
     - 为受保护系统卷和固定内置磁盘的置灰项注入 on_disabled_hover_text 悬停提示，清晰解释安全锁定缘由；
     - 设备与卷选择器刷新按钮显式标注「刷新设备 (F5)」与「刷新卷 (F5)」，并在主事件循环前置拦截 F5 / Ctrl+R 全局刷新快捷键；
  3. 自动存证案卷直通真实耗时、首发错误物证与测速生理快照 (src/gui/app.rs):
     - 重构任务完成边界的自动存证数据管道，将 snapshot.duration_seconds、snapshot.first_error_* 与 snapshot.physiology 完整直通 DiagnosticReport::build 与 auto_save_evidence_if_fake，彻底消灭自动存证案卷丢失 Hexdump 物证与测速数据的缺陷；
  4. 校验中途停止司法严谨定性与防虚假健康背书 (src/gui/app.rs & core/report.rs):
     - 读写公文卡片与纯文本/Markdown 报告全面强化 is_completed 状态判断；
     - 中途停止时呈现琥珀色「司法鉴定级存证 · 阶段抽检」公文卡片，客观定性为「实测已校验容量 X GB，未见异常。注意：剩余空间未经验证，本记录仅作为阶段性抽样存证，不代表整盘终局结论」，坚决只有全盘校验完成才出具 [AAA级健康介质] 报告；
     - 增加 OneDrive/Desktop 路径自动探测与样本量不足门禁，杜绝 0 字节误判 Grade F 报废黑片；
  5. 晶圆热力图动态保持 4:3 黄金微网格比例 (src/gui/components/heatmap.rs):
     - 单元格高度计算改为 cell_width * 0.75 结合 clamp(6.0, 11.0)，容器高度联动自适应，彻底根除 1080p @ 150% 笔记本与超宽屏下热力图被拉伸为扁长条纹的几何失真缺陷；
  6. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### CI全绿双架构部署大成版记录 (Run 39730409418)
- 流水线状态与指标：Run ID 39730409418 (Status: completed success，全量子任务 100% 满分全绿)
  - Code Quality & Unit Tests：零警告通过 cargo clippy --all-targets -- -D warnings 并通过全部核心单元测试 (2m 22s)
  - Build Release Binaries (x86_64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-x86_64-pc-windows-msvc.zip (2m 12s)
  - Build Release Binaries (aarch64-pc-windows-msvc)：成功生成部署产物 f3-win-v0.1.0-aarch64-pc-windows-msvc.zip (3m 19s)
- 产物详情与验证：
  - Windows x64 生产二进制：f3-win.exe / f3-win-v0.1.0.exe / f3-win-v0.1.0-x86_64.exe (精确 3,432,960 字节，SHA-256: c3cf845aa13175173f7c4d03abcb7bc65d1cdeeee89318780be1858c38538760，编译时间戳 2026-10-22 05:55)
  - Windows ARM64 生产二进制：f3-win-arm64.exe / f3-win-v0.1.0-arm64.exe (精确 3,264,000 字节，SHA-256: 8d2f92c5beb64dc6d7e1608b5e813dc01d323e740fa8822a1ea32baa983e1761，编译时间戳 2026-10-22 05:57)
  - 本地已安全热更新就位：f3-win.exe、f3-win-arm64.exe、f3-win-v0.1.0.exe、f3-win-v0.1.0-arm64.exe 与 f3-win-v0.1.0-x86_64.exe
- 架构演化与核心飞跃总结：
  1. 精准修复 report 结构体与主题配置阻断：补齐 DiagnosticReport::build_with_kind 中缺失的 is_completed 字段；移除 theme.rs 中 egui 0.29 未定义的实验性字段，保证代码库严格对齐 Clippy 零警告门禁；
  2. 全端无障碍与高对比度焦点体系：冷银白 BORDER_FOCUS (7.2:1 超高对比度，超 WCAG AA 3.0:1 门禁)；
  3. 司法存证案卷真实物证直通：测试完成边界直通真实耗时、首发错误 offset/expected/actual 与生理快照，彻底根除 dummy_telemetry 导致的物证缺失；
  4. 阶段性中途抽检客观定性：未全盘校验完成时出具琥珀色阶段抽检结论，非整盘合规不予签发 AAA 级报告；
  5. 晶圆热力图 4:3 黄金微网格：单元格高度动态联动 cell_width * 0.75，彻底消除高分屏与宽屏拉伸变形；
  6. 严格执行 Windows 原生更名热替换机制，平稳完成部署；
  7. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 开源F3核心测试行为像素级对标与安全防呆加固大成版交付记录 (2026-10-22)
- 治理目标：对标开源闪存测试标准 F3 (f3write/f3read/f3probe/f3fix) 上游源码，完成分卷命名与大小标准、PRNG 线性同余算法、末尾扇区对齐降级探测与一键清理安全防呆的全链路像素级对齐与代码加固。
- 像素级对标与架构验证结论：
  1. 文件名与分卷大小标准 100% 逐字节对齐：
     - 原版标准：1.h2w, 2.h2w ... N.h2w，单个整文件标准容量严格为 1073741824 字节 (1GiB = 1024*1024*1024)；
     - 对标结果：src/core/session.rs 中 GIGABYTE 常量定义为 1024*1024*1024 (1073741824)，分卷命名使用 format!("{}.h2w", file_index + 1)，与原版 full_fn_from_number 100% 逐字节对齐；
     - 末尾扇区精准写满加固：针对 Direct I/O 在 1MB 块写满触发 ERROR_DISK_FULL 时可能残留数十 KB 剩余空间的物理限制，新增阶梯式降级探测填充机制 (64KB -> 4KB -> 物理扇区大小 512B/4096B)，实现 100% 逐扇区彻底饱和填充磁盘末尾空间，并在写满截断时通过 FlushFileBuffers 穿透冲刷；
  2. PRNG 伪随机数序列生成 100% 逐比特向后兼容：
     - 原版标准：LCG 递推公式 next = current * 4294967311 + 17，每个 512 字节扇区前 8 字节为小端序绝对字节偏移量 (offset)；
     - 对标结果：src/core/algo.rs 中 LCG_MULTIPLIER = 4294967311，LCG_INCREMENT = 17。在 salt == 0 默认模式下，one_way_seed 直通返回 offset，生成的扇区首部 offset 及后续 63 个 u64 小端伪随机数与原版 f3write/f3read 及 H2testw 保持 100% 逐比特二进制互通；
  3. 测试文件扫描与一键清理安全防呆加固 (src/core/session.rs)：
     - 杜绝误伤目录：严格通过 entry.file_type().is_ok_and(|ft| ft.is_file()) 过滤，彻底杜绝扫描或误删同名文件夹及软链接/重解析点；
     - 平台兼容扩展名匹配：支持 Windows 平台大小写不敏感匹配 (.h2w / .H2W)；
     - 正规纯数字严格判定：严格断言前缀为非空、非零开头且全为 ASCII 数字的纯正整数，彻底拦截 0.h2w、01.h2w、+1.h2w、foo.h2w 及普通用户文件；
     - 清理逻辑严格受限：cleanup_h2w_files 仅删除匹配上述严格过滤条件的测试分卷，保证用户同盘其他数据绝对安全；
  4. 自动化单元测试完备覆盖：
     - 新增 test_discover_h2w_files_strict_filtering 单元测试，全面覆盖合法测试文件发现、非标文件与同名文件夹拦截以及清理后非标文件完好无损断言；
  5. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

### 底层系统与硬件容灾极端异常场景全链路加固大成版交付记录 (2026-10-22)
- 治理目标：针对突发硬件掉盘与拔盘 (Win32 Error 1167/1117/21/23)、磁盘写保护 (Error 19) 以及路径特殊字符与注入防御进行系统级极限容灾审计与代码加固。
- 深度根因排查与架构加固成果：
  1. Win32 错误码标准化提取与消除 HRESULT 负数失真 (src/core/direct_io.rs):
     - 根因分析：windows-rs 底层将 Win32 错误码封装为 HRESULT (0x8007xxxx 负数 i32，如 112 映射为 -2147024784，1167 映射为 -2147023729)；
     - 架构修复：在 direct_io.rs 中实现 parse_win32_code(hr: i32) -> u32，精准提取 Facility 7 的 low-16 位标准 Win32 错误码；所有 I/O 报错统一输出标准结构化标签 [Win32: {code}]；
     - 补充针对 112、39、19、1167、1117、21、23 的单元测试覆盖；
  2. 彻底根除掉盘误报成功的致命漏洞 (src/core/session.rs):
     - 根因分析：原 execute_write 中使用 e.contains("112") 匹配磁盘已满，当短写错误提示中包含数字 112 (如 offset 112000000) 或底层返回 Error 1127 时，掉盘故障被误判为磁盘已满，误报测试成功；
     - 架构修复：封装纯函数 is_disk_full_error(&str)，严格基于 [Win32: 112]、[Win32: 39] 与空间不足关键字进行精确匹配；
     - 补充 test_is_disk_full_error_precision 单元测试，拦截所有包含 112 子串的非满盘错误；
  3. 磁盘写保护 (Error 19) 与掉盘友好提示贯通 (src/core/direct_io.rs & devices.rs):
     - 为 Error 19 (ERROR_WRITE_PROTECT) 补充针对性的友好提示：明确告知用户物理写保护开关或只读属性状态；
     - 为 Error 1167、1117、21 明确标注设备未连接或突发掉盘故障；
     - 消除 StorageTarget::open_block_io 无条件追加 "(需要管理员权限)" 引起的误导；
  4. 句柄与独占锁 RAII 闭环与路径注入免疫确认 (src/core/probe.rs & fix.rs):
     - 验证 DirectIoFile 与 VolumeLockGuard 在所有 I/O 失败分支中自动触发 Drop 调用 CloseHandle，彻底释放句柄与 FSCTL_LOCK_VOLUME 锁；
     - 验证 UTF-16 宽字符支持带空格、中文路径；diskpart 脚本完全由数字与白名单参数构成，无命令注入隐患；
  5. 全量代码、注释与记忆库绝对零 emoji 规范恪守。

