#![allow(clippy::collapsible_match)]
//! F3-Win 纯标准库零依赖无头 CLI 引擎与子命令分发器 (Dual-mode CLI Engine)
//!
//! 深度实现与上游 f3 官方工具链 (f3write, f3read, f3probe, f3fix, f3brew) 100% 兼容的控制台模式，
//! 兼备动态父控制台吸附 (AttachConsole)、按需特权审查与平滑终端 ASCII 进度渲染。

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::algo::SECTOR_SIZE;
use crate::core::brew::{run_brew_benchmark, BrewConfig};
use crate::core::devices::{verify_target_path, PhysicalDeviceInfo, VolumeInfo};
use crate::core::direct_io::DirectIoFile;
use crate::core::fix::{F3FixPlan, SafePartitionFixer};
use crate::core::probe::{F3Probe, F3ProbeConfig, ProbeResult, ProbeTrap};
use crate::core::session::{H2wConfig, H2wMode, H2wSession, H2wTelemetry};

/// 动态吸附调用者的控制台窗口 (若从 cmd.exe 或 PowerShell 启动)
/// 如果是从资源管理器双击启动，则静默失败，保持纯净桌面 GUI 无黑框闪烁
pub fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
    }
    unsafe {
        let _ = AttachConsole(0xFFFF_FFFF); // ATTACH_PARENT_PROCESS
    }
}

/// 审查当前进程是否具备 Windows 管理员特权
pub fn is_admin() -> bool {
    use windows::Win32::UI::Shell::IsUserAnAdmin;
    unsafe { IsUserAnAdmin().as_bool() }
}

/// 格式化字节大小为人类可读字符串
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TB", b / TB)
    } else if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// 格式化耗时秒数为 HH:MM:SS 或 MM:SS
pub fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

/// 渲染控制台单行 ASCII 进度条
fn render_progress_line(
    phase: &str,
    pct: f32,
    speed_bps: f64,
    eta_secs: u64,
    extra: &str,
) {
    let bar_width: usize = 24;
    let clamped_pct = pct.clamp(0.0, 1.0);
    let filled = (clamped_pct * bar_width as f32).round() as usize;
    let empty = bar_width.saturating_sub(filled);
    let bar: String = "=".repeat(filled) + &"-".repeat(empty);

    let speed_mb = speed_bps / (1024.0 * 1024.0);
    let eta_str = if eta_secs > 0 {
        format_duration(eta_secs)
    } else {
        "--:--".to_string()
    };

    print!(
        "\r[{}] [{}] {:5.1}% | {:6.1} MB/s | ETA: {} {}",
        phase,
        bar,
        clamped_pct * 100.0,
        speed_mb,
        eta_str,
        extra
    );
    let _ = io::stdout().flush();
}

/// CLI 分发入口
/// 返回值：
/// - Some(exit_code)：已作为命令行工具执行完成，应直接使用该状态码退出进程
/// - None：无命令行参数或用户请求 GUI 模式，应平滑进入 GUI 界面
pub fn dispatch() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1 {
        return None;
    }

    // 发现命令行参数，即刻附着父控制台
    attach_console();

    // 提取进程名称，以支持通过重命名为 f3write.exe / f3read.exe 进行透明兼容调用
    let exe_name = Path::new(&args[0])
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (subcmd, rest_args) = if exe_name.contains("f3write") {
        ("write", &args[1..])
    } else if exe_name.contains("f3read") {
        ("read", &args[1..])
    } else if exe_name.contains("f3probe") {
        ("probe", &args[1..])
    } else if exe_name.contains("f3fix") {
        ("fix", &args[1..])
    } else if exe_name.contains("f3brew") {
        ("brew", &args[1..])
    } else {
        (args[1].as_str(), &args[2..])
    };

    match subcmd {
        "--gui" => None,
        "-h" | "--help" | "help" => {
            print_help();
            Some(0)
        }
        "-v" | "--version" | "version" => {
            print_version();
            Some(0)
        }
        "write" | "f3write" => Some(run_cmd_write(rest_args)),
        "read" | "f3read" => Some(run_cmd_read(rest_args)),
        "probe" | "f3probe" => Some(run_cmd_probe(rest_args)),
        "fix" | "f3fix" => Some(run_cmd_fix(rest_args)),
        "brew" | "f3brew" => Some(run_cmd_brew(rest_args)),
        "list" | "ls" => Some(run_cmd_list()),
        unknown => {
            eprintln!("错误: 未知子命令 '{}'。运行 'f3-win --help' 查看可用指令。", unknown);
            Some(1)
        }
    }
}

/// 打印主帮助文档
fn print_help() {
    println!(
r#"F3-Win (Fight Flash Fraud for Windows) v0.2.1
极速闪存扩容盘检测、物理探针与分区自愈修复工具

用法:
    f3-win <子命令> [参数...]
    或者将可执行文件重命名为 f3write.exe / f3read.exe / f3probe.exe / f3fix.exe / f3brew.exe 直接调用

子命令列表:
    write, f3write    向目标目录填充 .h2w 连续分卷测试文件 (文件系统级)
    read,  f3read     校验目标目录中的 .h2w 分卷，排查损坏与覆写扇区
    probe, f3probe    对物理裸盘执行双相几何跳跃探测，秒级测出真实物理容量 (需管理员权限)
    fix,   f3fix      根据真实容量安全重建 MBR/GPT 分区表并格式化 (需管理员权限)
    brew,  f3brew     执行闪存块级 I/O 延迟采样，输出微秒级时延直方图与 SLC 断崖诊断
    list,  ls         列出当前系统中所有的物理磁盘、逻辑卷及安全状态
    --gui             启动原生桌面图形界面

全局选项:
    -h, --help        显示此帮助信息
    -v, --version     显示版本信息

各子命令详细用法:
    f3-win write <目录路径> [--max-size <GB>] [--salt <数值>] [--chunk-size <MB>]
    f3-win read  <目录路径> [--salt <数值>] [--auto-cleanup]
    f3-win probe <物理盘号或路径> [--force]
    f3-win fix   <物理盘号> [--real-size <字节数>] [--fs <exfat|fat32|ntfs>] [--dry-run] [-y]
    f3-win brew  <目录路径> [--blocks <数量>] [--size <KB>] [--no-read]

示例:
    f3-win write E:    f3-win write E: --max-size 32 --salt 0x12345678
    f3-win read E: --auto-cleanup
    f3-win probe 1
    f3-win fix 1 --real-size 32212254720 -y
    f3-win brew E: --blocks 1024
"#);
}

/// 打印版本信息
fn print_version() {
    println!("f3-win v0.2.1 (Windows Native Rust + Direct I/O)");
    println!("源码仓库: https://github.com/Wenaixi/f3-win");
    println!("兼容标准: f3write/f3read/f3probe/f3fix/f3brew (upstream 100% compatible)");
}

/// 子命令: write
fn run_cmd_write(args: &[String]) -> i32 {
    if args.is_empty() || args[0].starts_with('-') {
        eprintln!("用法: f3-win write <目录路径> [--max-size <GB>] [--salt <数值>] [--chunk-size <MB>]");
        return 1;
    }

    let target_dir = PathBuf::from(&args[0]);
    if let Err(e) = verify_target_path(&target_dir) {
        eprintln!("安全拦截失败: {}", e);
        return 1;
    }

    let mut max_gb: Option<u64> = None;
    let mut salt: u64 = 0;
    let mut chunk_mb: usize = 1;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--max-size" if i + 1 < args.len() => {
                if let Ok(val) = args[i + 1].parse::<u64>() {
                    max_gb = Some(val);
                }
                i += 1;
            }
            "--salt" if i + 1 < args.len() => {
                let s = &args[i + 1];
                let parsed = if s.starts_with("0x") || s.starts_with("0X") {
                    u64::from_str_radix(&s[2..], 16)
                } else {
                    s.parse::<u64>()
                };
                if let Ok(val) = parsed {
                    salt = val;
                }
                i += 1;
            }
            "--chunk-size" if i + 1 < args.len() => {
                if let Ok(val) = args[i + 1].parse::<usize>() {
                    chunk_mb = val.clamp(1, 64);
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    println!("=== F3-Win 写入测试 (f3write) ===");
    println!("目标路径: {:?}", target_dir);
    println!("最大限制: {}", max_gb.map(|g| format!("{} GB", g)).unwrap_or_else(|| "写满可用空间".to_string()));
    println!("加盐种子: 0x{:016X} ({})", salt, if salt == 0 { "上游 H2W 兼容模式" } else { "不可逆 PRF 防作弊" });
    println!("块直通大小: {} MB", chunk_mb);
    println!("正在启动 Direct I/O 硬件直通写入...");

    let max_bytes = max_gb.map(|g| g * 1024 * 1024 * 1024);
    let config = H2wConfig {
        target_dir,
        mode: H2wMode::Write,
        start_file_num: 1,
        end_file_num: None,
        max_bytes,
        chunk_size: chunk_mb * 1024 * 1024,
        salt,
        auto_cleanup: false,
    };

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let session = H2wSession::new(config, cancel_flag);
    let (tx, rx) = crossbeam_channel::unbounded();

    std::thread::spawn(move || {
        session.run(tx);
    });

    let mut last_telemetry: Option<H2wTelemetry> = None;
    while let Ok(t) = rx.recv() {
        if !t.is_finished {
            let pct = if t.total_bytes_expected > 0 {
                t.bytes_processed as f32 / t.total_bytes_expected as f32
            } else {
                0.0
            };
            let extra = format!(
                "(分卷 {}{})",
                t.current_file,
                t.total_files.map(|tf| format!("/{}", tf)).unwrap_or_default()
            );
            render_progress_line("写入中", pct, t.speed_bps, t.eta_seconds, &extra);
        }
        last_telemetry = Some(t);
    }
    println!(); // 换行

    if let Some(t) = last_telemetry {
        if let Some(err) = t.error {
            eprintln!("写入测试异常终止: {}", err);
            return 1;
        }
        println!("=== 写入完成 ===");
        println!("写入数据总量: {}", format_bytes(t.bytes_processed));
        println!("耗时: {}", format_duration(t.duration_seconds.round() as u64));
        let avg_speed = if t.duration_seconds > 0.0 {
            t.bytes_processed as f64 / t.duration_seconds / (1024.0 * 1024.0)
        } else {
            0.0
        };
        println!("平均写入速度: {:.2} MB/s", avg_speed);
        0
    } else {
        eprintln!("错误: 未收到遥测状态");
        1
    }
}

/// 子命令: read
fn run_cmd_read(args: &[String]) -> i32 {
    if args.is_empty() || args[0].starts_with('-') {
        eprintln!("用法: f3-win read <目录路径> [--salt <数值>] [--auto-cleanup]");
        return 1;
    }

    let target_dir = PathBuf::from(&args[0]);
    if let Err(e) = verify_target_path(&target_dir) {
        eprintln!("安全拦截失败: {}", e);
        return 1;
    }

    let mut salt: u64 = 0;
    let mut auto_cleanup = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--salt" if i + 1 < args.len() => {
                let s = &args[i + 1];
                let parsed = if s.starts_with("0x") || s.starts_with("0X") {
                    u64::from_str_radix(&s[2..], 16)
                } else {
                    s.parse::<u64>()
                };
                if let Ok(val) = parsed {
                    salt = val;
                }
                i += 1;
            }
            "--auto-cleanup" => {
                auto_cleanup = true;
            }
            _ => {}
        }
        i += 1;
    }

    println!("=== F3-Win 校验测试 (f3read) ===");
    println!("目标路径: {:?}", target_dir);
    println!("加盐种子: 0x{:016X}", salt);
    println!("自动清理: {}", if auto_cleanup { "开启 (测试成功后清除分卷)" } else { "关闭" });

    let files = H2wSession::discover_h2w_files(&target_dir);
    if files.is_empty() {
        eprintln!("错误: 目标目录下未发现任何可用的 .h2w 测试分卷文件。请先运行 f3write 进行数据填充。");
        return 1;
    }
    println!("发现 {} 个 .h2w 测试分卷文件，开始校验...", files.len());

    let config = H2wConfig {
        target_dir,
        mode: H2wMode::Verify,
        start_file_num: 1,
        end_file_num: None,
        max_bytes: None,
        chunk_size: 1024 * 1024,
        salt,
        auto_cleanup,
    };

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let session = H2wSession::new(config, cancel_flag);
    let (tx, rx) = crossbeam_channel::unbounded();

    std::thread::spawn(move || {
        session.run(tx);
    });

    let mut last_telemetry: Option<H2wTelemetry> = None;
    while let Ok(t) = rx.recv() {
        if !t.is_finished {
            let pct = if t.total_bytes_expected > 0 {
                t.bytes_processed as f32 / t.total_bytes_expected as f32
            } else {
                0.0
            };
            let extra = format!(
                "(损坏: {}, 覆写: {})",
                t.stats.bad,
                t.stats.overwritten
            );
            render_progress_line("校验中", pct, t.speed_bps, t.eta_seconds, &extra);
        }
        last_telemetry = Some(t);
    }
    println!();

    if let Some(t) = last_telemetry {
        if let Some(err) = t.error {
            eprintln!("校验测试异常中断: {}", err);
            return 1;
        }

        println!("=== 校验完成报告 ===");
        println!("校验数据总量: {}", format_bytes(t.bytes_processed));
        println!("耗时: {}", format_duration(t.duration_seconds.round() as u64));
        let avg_speed = if t.duration_seconds > 0.0 {
            t.bytes_processed as f64 / t.duration_seconds / (1024.0 * 1024.0)
        } else {
            0.0
        };
        println!("平均读取速度: {:.2} MB/s", avg_speed);
        println!("正确数据扇区: {} 扇区 ({})", t.stats.good, format_bytes(t.stats.good * SECTOR_SIZE as u64));
        println!("损坏数据扇区: {} 扇区 ({})", t.stats.bad, format_bytes(t.stats.bad * SECTOR_SIZE as u64));
        println!("覆写数据扇区: {} 扇区 ({})", t.stats.overwritten, format_bytes(t.stats.overwritten * SECTOR_SIZE as u64));

        if t.stats.bad > 0 || t.stats.overwritten > 0 {
            println!();
            println!("【判定结果: 扩容假盘或物理损坏 (FAILED)】");
            if let Some(offset) = t.first_error_offset {
                let file_str = t.first_error_file.map(|f| format!("{}.h2w", f)).unwrap_or_else(|| "未知".to_string());
                println!("首发错误位置: 文件 {}，偏移量 0x{:08X} ({})", file_str, offset, format_bytes(offset));
            }
            if let (Some(exp), Some(act)) = (t.first_error_expected, t.first_error_actual) {
                println!("预期数据摘要: {:02X?}", exp);
                println!("实际读回摘要: {:02X?}", act);
            }
            1
        } else {
            println!();
            println!("【判定结果: 合格健康闪存介质 (PASSED)】");
            if auto_cleanup {
                println!("测试分卷文件已安全自动清理。");
            }
            0
        }
    } else {
        eprintln!("错误: 未收到遥测状态");
        1
    }
}

/// 解析物理磁盘盘号
fn parse_disk_index(s: &str) -> Result<u32, String> {
    let s_clean = s.trim();
    if let Ok(idx) = s_clean.parse::<u32>() {
        return Ok(idx);
    }
    let upper = s_clean.to_uppercase();
    if upper.contains("PHYSICALDRIVE") {
        if let Some(pos) = upper.find("PHYSICALDRIVE") {
            let num_part = &upper[pos + "PHYSICALDRIVE".len()..];
            if let Ok(idx) = num_part.trim_matches(|c: char| !c.is_ascii_digit()).parse::<u32>() {
                return Ok(idx);
            }
        }
    }
    Err(format!(r"无法解析物理磁盘编号: '{}'。示例: '1' 或 '\.PhysicalDrive1'", s))
}

/// 子命令: probe
fn run_cmd_probe(args: &[String]) -> i32 {
    if args.is_empty() || args[0].starts_with('-') {
        eprintln!("用法: f3-win probe <物理盘号或路径> [--force]");
        eprintln!("示例: f3-win probe 1");
        return 1;
    }

    if !is_admin() {
        eprintln!("错误: f3probe 需要 Windows 管理员特权以直接访问底层物理裸盘与扇区。");
        eprintln!("请在以管理员身份运行的命令提示符 (cmd) 或 PowerShell 窗口中重试。");
        return 1;
    }

    let disk_index = match parse_disk_index(&args[0]) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    let force = args.iter().any(|a| a == "--force");

    println!("=== F3-Win 物理跳跃探针 (f3probe) ===");
    println!("正在查询 PhysicalDrive{} 硬件属性...", disk_index);

    let dev_info = match PhysicalDeviceInfo::query_by_index(disk_index) {
        Some(info) => info,
        None => {
            eprintln!("错误: 找不到 PhysicalDrive{}，请确认磁盘编号是否正确且介质未断开。", disk_index);
            return 1;
        }
    };

    println!("设备名称: {}", dev_info.display_name());
    println!("标称容量: {}", format_bytes(dev_info.total_bytes));
    println!("总线类型: {}", dev_info.bus_type.display_name());

    if disk_index == 0 {
        eprintln!("安全硬拦截：PhysicalDrive0 为系统主盘，绝对禁止执行底层覆写测试！");
        return 1;
    }

    if !dev_info.is_safe_target && !force {
        eprintln!("安全拦截：该驱动器未标记为安全的移动存储介质 (USB/SD/MMC)。");
        eprintln!("若您十分确定要在该介质上执行测试，请追加 '--force' 显式跳过总线白名单检查。");
        return 1;
    }

    println!();
    println!("【警告】探针测试将直接对底层物理扇区进行采样测试，测试完成后请格式化该磁盘！");
    println!("开始执行双相几何跳跃探针探测...");

    let total_sectors = dev_info.total_bytes / 512;
    let config = F3ProbeConfig { total_sectors };
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let probe = F3Probe::new(config, cancel_flag);

    let dev_path = format!(r"\.\PhysicalDrive{}", disk_index);
    let mut dev = match DirectIoFile::open_raw_device(Path::new(&dev_path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("无法打开物理设备: {}", e);
            return 1;
        }
    };

    let (tx, rx) = crossbeam_channel::unbounded();

    let probe_thread = std::thread::spawn(move || {
        probe.run(&mut dev, tx)
    });

    while let Ok(t) = rx.recv() {
        if !t.is_finished {
            let extra = format!("阶段: {}", t.current_phase);
            render_progress_line("探针测试", t.progress_pct, 0.0, 0, &extra);
        }
    }
    println!();

    match probe_thread.join() {
        Ok(Ok(res)) => {
            print_probe_report(&res);
            if res.is_fake { 2 } else { 0 }
        }
        Ok(Err(e)) => {
            eprintln!("探针执行失败: {}", e);
            1
        }
        Err(_) => {
            eprintln!("探针线程异常崩溃");
            1
        }
    }
}

/// 打印探针结果报告
fn print_probe_report(res: &ProbeResult) {
    println!("=== 探针探测诊断报告 ===");
    println!("标称容量: {} ({} 扇区)", format_bytes(res.announced_size), res.announced_size / 512);
    println!("真实物理容量: {} ({} 扇区)", format_bytes(res.real_size), res.real_size / 512);

    match res.trap {
        ProbeTrap::None => {
            println!("陷阱类型: 无 (健康正品闪存)");
            println!();
            println!("【最终判定: 合格正品】物理颗粒容量与主控标称容量完全吻合。");
        }
        ProbeTrap::BitBucket => {
            println!("陷阱类型: 位黑洞 (BitBucket - 写入静默丢弃，回读全 0/全 FF 或损坏)");
            println!();
            println!("【最终判定: 扩容假盘】实测物理容量仅为标称的 {:.1}%！", (res.real_size as f64 / res.announced_size as f64) * 100.0);
            println!("建议运行: f3-win fix <盘号> --real-size {} 以恢复真实可用分区", res.real_size);
        }
        ProbeTrap::WrapAround => {
            println!("陷阱类型: 物理地址回绕 (WrapAround - 高位写入循环覆盖低位真实扇区)");
            println!();
            println!("【最终判定: 扩容假盘】实测物理容量仅为标称的 {:.1}%！", (res.real_size as f64 / res.announced_size as f64) * 100.0);
            println!("建议运行: f3-win fix <盘号> --real-size {} 以恢复真实可用分区", res.real_size);
        }
    }
}

/// 子命令: fix
fn run_cmd_fix(args: &[String]) -> i32 {
    if args.is_empty() || args[0].starts_with('-') {
        eprintln!("用法: f3-win fix <物理盘号> [--real-size <字节数>] [--fs <exfat|fat32|ntfs>] [--dry-run] [-y]");
        eprintln!("示例: f3-win fix 1 --real-size 32212254720 -y");
        return 1;
    }

    if !is_admin() {
        eprintln!("错误: f3fix 需要 Windows 管理员特权以重写磁盘分区表并调用 diskpart。");
        eprintln!("请在以管理员身份运行的命令提示符或 PowerShell 窗口中重试。");
        return 1;
    }

    let disk_index = match parse_disk_index(&args[0]) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    let mut real_size: Option<u64> = None;
    let mut file_system: Option<String> = None;
    let mut dry_run = false;
    let mut assume_yes = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--real-size" => {
                if i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<u64>() {
                        real_size = Some(val);
                    }
                    i += 1;
                }
            }
            "--fs" => {
                if i + 1 < args.len() {
                    file_system = Some(args[i + 1].to_lowercase());
                    i += 1;
                }
            }
            "--dry-run" => dry_run = true,
            "-y" | "--yes" => assume_yes = true,
            _ => {}
        }
        i += 1;
    }

    println!("=== F3-Win 分区自愈修复 (f3fix) ===");
    let dev_info = match PhysicalDeviceInfo::query_by_index(disk_index) {
        Some(d) => d,
        None => {
            eprintln!("错误: 找不到 PhysicalDrive{}", disk_index);
            return 1;
        }
    };

    let real_bytes = match real_size {
        Some(b) => b,
        None => {
            eprintln!("错误: 未指定真实物理容量 (--real-size <字节数>)。");
            eprintln!("建议先运行 'f3-win probe {}' 获得准确的真实物理字节数！", disk_index);
            return 1;
        }
    };

    let mut plan = F3FixPlan::new(disk_index, dev_info.total_bytes, real_bytes);
    if let Some(fs) = file_system {
        plan.file_system = fs;
    }

    let executor = SafePartitionFixer::new(plan);

    if dry_run {
        println!("正在生成预演脚本 (Dry-Run)...");
        match executor.dry_run() {
            Ok(report) => {
                println!("=== 修复方案预演报告 ===");
                println!("目标磁盘: PhysicalDrive{}", report.disk_index);
                println!("设备型号: {} {}", report.vendor_id, report.product_id);
                println!("总线类型: {}", report.bus_name);
                println!("安全分区容量: {} MB ({})", report.safe_partition_mb, format_bytes(report.safe_partition_mb * 1024 * 1024));
                println!("目标文件系统: {}", report.file_system);
                println!("--- 生成的 Diskpart 脚本 ---");
                println!("{}", report.script_content);
                return 0;
            }
            Err(e) => {
                eprintln!("安全合规验证失败: {}", e);
                return 1;
            }
        }
    }

    // 交互确认
    if !assume_yes {
        println!("目标设备: {}", dev_info.display_name());
        println!("安全分区目标大小: {} MB", executor.plan.safe_partition_mb);
        println!("目标文件系统: {}", executor.plan.file_system);
        println!();
        print!("【警告】此操作将清除 PhysicalDrive{} 上的所有数据并重写分区表！确认继续? (y/N): ", disk_index);
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() || !input.trim().eq_ignore_ascii_case("y") {
            println!("用户取消操作。");
            return 0;
        }
    }

    println!("正在应用安全分区修复...");
    match executor.apply() {
        Ok(out) => {
            println!("=== 分区修复成功 ===");
            println!("{}", out);
            0
        }
        Err(e) => {
            eprintln!("分区修复失败: {}", e);
            1
        }
    }
}

/// 子命令: brew
fn run_cmd_brew(args: &[String]) -> i32 {
    if args.is_empty() || args[0].starts_with('-') {
        eprintln!("用法: f3-win brew <目录路径> [--blocks <数量>] [--size <KB>] [--no-read]");
        eprintln!(r"示例: f3-win brew E:\ --blocks 1024");
        return 1;
    }

    let target_dir = PathBuf::from(&args[0]);
    if let Err(e) = verify_target_path(&target_dir) {
        eprintln!("安全拦截失败: {}", e);
        return 1;
    }

    let mut blocks: usize = 1024;
    let mut block_size_kb = 64;
    let mut test_read = true;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--blocks" if i + 1 < args.len() => {
                if let Ok(val) = args[i + 1].parse::<usize>() {
                    blocks = val.clamp(16, 65536);
                }
                i += 1;
            }
            "--size" if i + 1 < args.len() => {
                if let Ok(val) = args[i + 1].parse::<usize>() {
                    block_size_kb = val.clamp(4, 4096);
                }
                i += 1;
            }
            "--no-read" => {
                test_read = false;
            }
            _ => {}
        }
        i += 1;
    }

    let block_size = block_size_kb * 1024;
    println!("=== F3-Win 闪存硬件时延画像 (f3brew) ===");
    println!("目标路径: {:?}", target_dir);
    println!("块大小: {} KB ({} 字节)", block_size_kb, block_size);
    println!("总块数: {} (总量: {})", blocks, format_bytes((blocks as u64) * block_size as u64));
    println!("读校验: {}", if test_read { "开启" } else { "跳过" });
    println!("正在启动 Direct I/O 无缓存延迟画像采样...");

    let config = BrewConfig {
        target_dir,
        block_size,
        total_blocks: blocks,
        test_read,
        test_filename: "f3_brew_benchmark.tmp".to_string(),
    };

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::unbounded();

    let benchmark_thread = std::thread::spawn(move || {
        run_brew_benchmark(&config, cancel_flag, move |p| {
            let _ = tx.send(p);
        })
    });

    while let Ok(progress) = rx.recv() {
        let phase_str = match progress.phase {
            crate::core::brew::BrewPhase::Writing => "写入采样",
            crate::core::brew::BrewPhase::Flushing => "冲刷缓存",
            crate::core::brew::BrewPhase::Reading => "回读采样",
            crate::core::brew::BrewPhase::Completed => "分析完成",
        };
        let pct = if progress.total_blocks > 0 {
            progress.current_block as f32 / progress.total_blocks as f32
        } else {
            0.0
        };
        let extra = format!("(块 {}/{})", progress.current_block, progress.total_blocks);
        render_progress_line(phase_str, pct, 0.0, 0, &extra);
    }
    println!();

    match benchmark_thread.join() {
        Ok(Ok(res)) => {
            println!("{}", res.format_report());
            0
        }
        Ok(Err(e)) => {
            eprintln!("时延画像测试失败: {}", e);
            1
        }
        Err(_) => {
            eprintln!("采样线程崩溃");
            1
        }
    }
}

/// 子命令: list
fn run_cmd_list() -> i32 {
    println!("=== F3-Win 存储设备与逻辑卷清单 ===");
    println!();
    println!("--- 物理磁盘 (Physical Drives) ---");
    let drives = PhysicalDeviceInfo::enumerate_devices();
    if drives.is_empty() {
        println!("(未检测到物理磁盘)");
    } else {
        for d in &drives {
            let safe_tag = if d.is_safe_target {
                "[安全移动介质]"
            } else {
                "[锁定/固定介质]"
            };
            println!("  - #{}: {} | {} | {}", d.drive_index, d.display_name(), format_bytes(d.total_bytes), safe_tag);
        }
    }

    println!();
    println!("--- 可用逻辑卷 (Logical Volumes) ---");
    let volumes = VolumeInfo::enumerate_volumes();
    if volumes.is_empty() {
        println!("(未检测到可用逻辑卷)");
    } else {
        for v in &volumes {
            let safe_tag = if v.is_safe_target {
                "[安全目标]"
            } else {
                "[受限保护]"
            };
            let free_str = format!("{}/{}", format_bytes(v.free_bytes), format_bytes(v.total_bytes));
            println!(r"  - {}:\ ({}) | {} | {} | {}", v.drive_letter, v.file_system, free_str, v.display_name(), safe_tag);
        }
    }
    println!();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(5), "00:05");
        assert_eq!(format_duration(65), "01:05");
        assert_eq!(format_duration(3665), "01:01:05");
    }

    #[test]
    fn test_parse_disk_index() {
        assert_eq!(parse_disk_index("1").unwrap(), 1);
        assert_eq!(parse_disk_index("0").unwrap(), 0);
        assert_eq!(parse_disk_index("PhysicalDrive2").unwrap(), 2);
        assert_eq!(parse_disk_index(r"\\.\PhysicalDrive3").unwrap(), 3);
        assert!(parse_disk_index("invalid_disk").is_err());
    }
}
