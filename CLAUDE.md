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
