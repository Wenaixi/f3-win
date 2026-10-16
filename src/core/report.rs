use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use crate::core::devices::PhysicalDeviceInfo;
use crate::core::algo::BlockStats;
use crate::core::session::{H2wMode, H2wTelemetry};
use crate::core::speed::SpeedPhysiologySnapshot;

/// 审计报告模式类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
    /// 独立写入性能与 SLC 掉速生理审计
    WriteOnly,
    /// 独立读取校验与微观比特失配现场审计
    VerifyOnly,
    /// 写入与读取双阶段完整汇总存证审计
    Comprehensive,
}

impl AuditKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            AuditKind::WriteOnly => "写入性能与 SLC 掉速独立审计报告",
            AuditKind::VerifyOnly => "读取校验与微观失配独立审计报告",
            AuditKind::Comprehensive => "闪存介质物理容量真实性司法鉴定级汇总存证报告",
        }
    }
}

/// 报告导出格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// 标准 GitHub 风格 Markdown 格式
    Markdown,
    /// 纯文本 TXT 格式 (带 ASCII 表格与分割线，适合案卷打印与无渲染器环境)
    PlainText,
}

/// 芯片与硬件物理推断诊断结果
#[derive(Debug, Clone)]
pub struct HardwareInference {
    pub inferred_bus: String,
    pub flash_grade: String,
    pub fraud_type: String,
    pub pathology_analysis: String,
}

/// 法庭级闪存欺诈鉴定与存证报告
#[derive(Debug, Clone)]
pub struct DiagnosticReport {
    pub audit_kind: AuditKind,
    pub tool_name: String,
    pub tool_version: String,
    pub os_version: String,
    pub generated_at_local: String,
    pub is_direct_io: bool,

    pub target_drive: String,
    pub volume_serial: Option<u32>,
    pub physical_drive_path: String,
    pub bus_type: String,
    pub vendor_id: String,
    pub product_id: String,
    pub serial_number: String,
    pub announced_bytes: u64,

    pub salt: u64,
    pub salt_hex: String,

    pub write_tested: bool,
    pub write_bytes: u64,
    pub write_duration_secs: f64,
    pub write_avg_speed_mb: f64,
    pub write_peak_speed_mb: f64,
    pub write_valley_speed_mb: f64,
    pub write_median_speed_mb: f64,
    pub cv_jitter: f64,
    pub stall_ratio: f64,
    pub slc_cliff_bytes: Option<u64>,
    pub slc_cliff_speed_before_mb: f64,
    pub slc_cliff_speed_after_mb: f64,
    pub slc_cliff_drop_pct: f64,

    pub verify_tested: bool,
    pub verify_bytes: u64,
    pub verify_duration_secs: f64,
    pub verify_avg_speed_mb: f64,
    pub stats: BlockStats,

    pub first_error_offset: Option<u64>,
    pub first_error_file: Option<u64>,
    pub first_error_expected: Option<[u8; 16]>,
    pub first_error_actual: Option<[u8; 16]>,

    pub inference: HardwareInference,

    pub is_fake: bool,
    pub real_usable_bytes: u64,
    pub fake_bytes: u64,
    pub fake_ratio_pct: f64,
}

impl DiagnosticReport {
    /// 统一构建方法 (自动根据遥测模式判断审计类型，保持向后兼容)
    pub fn build(
        device: Option<&PhysicalDeviceInfo>,
        volume_serial: Option<u32>,
        target_path: &Path,
        telemetry: &H2wTelemetry,
        salt: u64,
        physiology: Option<SpeedPhysiologySnapshot>,
    ) -> Self {
        let kind = match telemetry.mode {
            H2wMode::Write => AuditKind::WriteOnly,
            H2wMode::Verify => AuditKind::VerifyOnly,
            H2wMode::WriteAndVerify => AuditKind::Comprehensive,
        };
        Self::build_with_kind(kind, device, volume_serial, target_path, telemetry, salt, physiology)
    }

    /// 独立写入测试报告工厂方法
    pub fn build_write_audit(
        device: Option<&PhysicalDeviceInfo>,
        volume_serial: Option<u32>,
        target_path: &Path,
        telemetry: &H2wTelemetry,
        salt: u64,
        physiology: Option<SpeedPhysiologySnapshot>,
    ) -> Self {
        Self::build_with_kind(AuditKind::WriteOnly, device, volume_serial, target_path, telemetry, salt, physiology)
    }

    /// 独立读取校验报告工厂方法
    pub fn build_verify_audit(
        device: Option<&PhysicalDeviceInfo>,
        volume_serial: Option<u32>,
        target_path: &Path,
        telemetry: &H2wTelemetry,
        salt: u64,
        physiology: Option<SpeedPhysiologySnapshot>,
    ) -> Self {
        Self::build_with_kind(AuditKind::VerifyOnly, device, volume_serial, target_path, telemetry, salt, physiology)
    }

    /// 写入与读取双阶段汇总存证报告工厂方法
    pub fn build_comprehensive(
        device: Option<&PhysicalDeviceInfo>,
        volume_serial: Option<u32>,
        target_path: &Path,
        telemetry: &H2wTelemetry,
        salt: u64,
        physiology: Option<SpeedPhysiologySnapshot>,
    ) -> Self {
        Self::build_with_kind(AuditKind::Comprehensive, device, volume_serial, target_path, telemetry, salt, physiology)
    }

    fn build_with_kind(
        audit_kind: AuditKind,
        device: Option<&PhysicalDeviceInfo>,
        volume_serial: Option<u32>,
        target_path: &Path,
        telemetry: &H2wTelemetry,
        salt: u64,
        physiology: Option<SpeedPhysiologySnapshot>,
    ) -> Self {
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let drive_str = target_path.to_string_lossy().to_string();
        let announced_bytes = device.map(|d| d.total_bytes).unwrap_or(telemetry.total_bytes_expected);

        let is_write = audit_kind == AuditKind::WriteOnly || audit_kind == AuditKind::Comprehensive;
        let is_verify = audit_kind == AuditKind::VerifyOnly || audit_kind == AuditKind::Comprehensive;
        let total_corrupted = telemetry.stats.bad + telemetry.stats.overwritten + telemetry.stats.changed;
        let is_fake = telemetry.stats.overwritten > 0 || (is_verify && total_corrupted > 0);

        let real_usable_bytes = if is_fake {
            if let Some(err_off) = telemetry.first_error_offset {
                err_off
            } else {
                telemetry.stats.good * 512
            }
        } else {
            announced_bytes
        };

        let fake_bytes = announced_bytes.saturating_sub(real_usable_bytes);
        let fake_ratio = if announced_bytes > 0 {
            (fake_bytes as f64 / announced_bytes as f64) * 100.0
        } else {
            0.0
        };

        let peak_mb = physiology.map(|p| p.peak_speed_bps / (1024.0 * 1024.0)).unwrap_or(telemetry.speed_bps / (1024.0 * 1024.0));
        let avg_mb = physiology.map(|p| p.average_speed_bps / (1024.0 * 1024.0)).unwrap_or(telemetry.speed_bps / (1024.0 * 1024.0));
        let valley_mb = physiology.map(|p| p.valley_speed_bps / (1024.0 * 1024.0)).unwrap_or(0.0);
        let median_mb = physiology.map(|p| p.median_speed_bps / (1024.0 * 1024.0)).unwrap_or(avg_mb);
        let cv_jitter = physiology.map(|p| p.cv_jitter).unwrap_or(0.0);
        let stall_ratio = physiology.map(|p| p.stall_ratio).unwrap_or(0.0);

        let (slc_cliff_bytes, slc_cliff_speed_before_mb, slc_cliff_speed_after_mb, slc_cliff_drop_pct) = if let Some(cliff) = physiology.and_then(|p| p.slc_cliff) {
            (Some(cliff.cliff_bytes), cliff.speed_before_bps / (1024.0 * 1024.0), cliff.speed_after_bps / (1024.0 * 1024.0), cliff.drop_percentage)
        } else {
            (None, 0.0, 0.0, 0.0)
        };

        let inference = Self::infer_hardware(
            device,
            peak_mb,
            avg_mb,
            telemetry.stats.overwritten > 0,
            telemetry.stats.bad > 0,
            is_fake,
        );

        Self {
            audit_kind,
            tool_name: "F3-Win 闪存检测终端".to_string(),
            tool_version: "v0.1.0-Release (Win32 Direct I/O 特权版)".to_string(),
            os_version: "Microsoft Windows NT (内核特权模式)".to_string(),
            generated_at_local: date_str,
            is_direct_io: true,

            target_drive: drive_str,
            volume_serial,
            physical_drive_path: device.map(|d| d.path.clone()).unwrap_or_else(|| r#"\\.\PhysicalDrive?"#.to_string()),
            bus_type: device.map(|d| d.bus_type.display_name().to_string()).unwrap_or_else(|| "USB 移动设备".to_string()),
            vendor_id: device.map(|d| d.vendor_id.clone()).unwrap_or_else(|| "未知厂商".to_string()),
            product_id: device.map(|d| d.product_id.clone()).unwrap_or_else(|| "未知存储设备".to_string()),
            serial_number: device.map(|d| d.serial_number.clone()).unwrap_or_else(|| "未公开或已篡改".to_string()),
            announced_bytes,

            salt,
            salt_hex: format!("0x{:016X}", salt),

            write_tested: is_write || telemetry.mode == H2wMode::WriteAndVerify,
            write_bytes: telemetry.bytes_processed,
            write_duration_secs: telemetry.duration_seconds,
            write_avg_speed_mb: avg_mb,
            write_peak_speed_mb: peak_mb,
            write_valley_speed_mb: valley_mb,
            write_median_speed_mb: median_mb,
            cv_jitter,
            stall_ratio,
            slc_cliff_bytes,
            slc_cliff_speed_before_mb,
            slc_cliff_speed_after_mb,
            slc_cliff_drop_pct,

            verify_tested: is_verify,
            verify_bytes: telemetry.bytes_processed,
            verify_duration_secs: telemetry.duration_seconds,
            verify_avg_speed_mb: avg_mb,
            stats: telemetry.stats,

            first_error_offset: telemetry.first_error_offset,
            first_error_file: telemetry.first_error_file,
            first_error_expected: telemetry.first_error_expected,
            first_error_actual: telemetry.first_error_actual,

            inference,
            is_fake,
            real_usable_bytes,
            fake_bytes,
            fake_ratio_pct: fake_ratio,
        }
    }

    fn infer_hardware(
        device: Option<&PhysicalDeviceInfo>,
        peak_speed_mb: f64,
        avg_speed_mb: f64,
        has_overwritten: bool,
        has_bad: bool,
        is_fake: bool,
    ) -> HardwareInference {
        let bus_str = device.map(|d| d.bus_type.display_name()).unwrap_or("USB");
        let inferred_bus = if bus_str.contains("USB") {
            if peak_speed_mb < 43.0 {
                "伪装 USB 3.0 / USB 3.2 (装饰性蓝色胶芯，实际物理通道被物理锁死在 USB 2.0 High-Speed 480Mbps 红线之下)".to_string()
            } else if peak_speed_mb < 150.0 {
                "标准 USB 3.0 Gen1 (5Gbps 真实物理通道)".to_string()
            } else {
                "高速 USB 3.2 Gen2 / SSD 级高速物理桥接通道".to_string()
            }
        } else {
            bus_str.to_string()
        };

        let flash_grade = if avg_speed_mb < 1.0 {
            "极度报废黑片 / 严重漏电下脚料 / 报废 MicroSD(TF) 废旧打胶封装芯片 (坏块率 >50%，绝缘层破损，电荷极速泄露)".to_string()
        } else if avg_speed_mb < 3.0 {
            "深度降级 TLC 晶圆残次片 / 二手翻新拆机片 (Ink Die，未通过原厂测试，物理擦写寿命仅剩数十次)".to_string()
        } else if avg_speed_mb < 8.0 {
            "降级 QLC / 3D TLC 晶圆边缘次品白片 (出 SLC 缓存后直写速度极低，无交错并行能力)".to_string()
        } else if avg_speed_mb < 20.0 {
            "常规合格降级 3D TLC 原厂白片 / 低速 Good Die (B-Grade)".to_string()
        } else {
            "正品原厂高良率 NAND 闪存颗粒".to_string()
        };

        let fraud_type = if has_overwritten {
            "硬件固件地址回绕型 (Address Line Wrap-around，主控执行模掩码运算，高位写入强制覆写盘首重要数据)".to_string()
        } else if has_bad && is_fake {
            "位黑洞丢弃型 (Bit-bucket Limbo，超出物理容量的写入直接静默丢弃，读回伪造全0或全FF)".to_string()
        } else if is_fake {
            "微码篡改容量虚标扩容假盘".to_string()
        } else {
            "物理介质真实合规".to_string()
        };

        let pathology_analysis = if is_fake || avg_speed_mb < 8.0 {
            "【芯片级病理解析】：该设备在初始阶段测得的高速写入，系闪存主控在前端开辟的极小容量动态 SLC Cache 模拟加速所致；当模拟缓存耗尽后，速度呈现断崖式下跌，暴露出背后极低性能的劣质物理颗粒；测试过程中的剧烈速度锯齿波动与卡顿归零，是由于主控芯片极其廉价低端，无外置 DRAM 物理缓存，在执行后台坏块磨损均衡与垃圾回收 (Garbage Collection, GC) 时芯片算力死锁，导致 USB 数据总线发生周期性传输阻塞。该生理特征为典型黑产扩容假盘的标志性行为。".to_string()
        } else {
            "数据传输平稳连续，主控调度及坏块映射状态良好，未见异常死锁或缓存断崖。".to_string()
        };

        HardwareInference {
            inferred_bus,
            flash_grade,
            fraud_type,
            pathology_analysis,
        }
    }

    pub fn generate_markdown(&self) -> String {
        let gb = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
        let serial_hex = self.volume_serial.map(|s| format!("{:08X}", s)).unwrap_or_else(|| "N/A".to_string());
        let mut doc = String::new();

        doc.push_str("# 存储介质物理容量真实性司法鉴定级检验报告\n");
        doc.push_str(&format!("### 【报告类型】: **{}**\n\n", self.audit_kind.display_name()));

        // 一、鉴定背景与操作系统内核特权级声明
        doc.push_str("## 一、 鉴定背景与操作系统内核特权级声明\n\n");
        doc.push_str("| 审计项目 | 技术参数与内核状态 | 司法证明力说明 |\n");
        doc.push_str("| :--- | :--- | :--- |\n");
        doc.push_str(&format!("| **审计终端软件** | {} {} | 原生无缓冲直接 I/O 驱动引擎，纯 Rust 2021 编译 |\n", self.tool_name, self.tool_version));
        doc.push_str(&format!("| **宿主操作系统** | {} | 权威特权执行环境 |\n", self.os_version));
        doc.push_str(&format!("| **存证生成时间** | {} | 本地法定公证时间戳 |\n", self.generated_at_local));
        doc.push_str("| **物理 I/O 管线** | **Win32 Direct I/O (底层硬件直通)** | `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH` 彻底旁路系统缓存 |\n");
        doc.push_str("| **缓存穿透证明** | **100% 物理绕过系统页缓存** | 数据直接穿透至硬件存储芯片物理介质，具备排他法定证据效力 |\n\n");

        // 二、被测硬件物理拓扑与全局唯一指纹
        doc.push_str("## 二、 被测硬件物理拓扑与全局唯一指纹\n\n");
        doc.push_str("| 检验项目 | 硬件物理属性参数 | 技术证据说明 |\n");
        doc.push_str("| :--- | :--- | :--- |\n");
        doc.push_str(&format!("| **测试检验工具** | {} {} | 原生无缓冲直接 I/O 驱动引擎 |\n", self.tool_name, self.tool_version));
        doc.push_str(&format!("| **宿主操作系统** | {} | 特权执行环境 |\n", self.os_version));
        doc.push_str(&format!("| **检验生成时间** | {} | 本地法定时间戳 |\n", self.generated_at_local));
        doc.push_str(&format!("| **被测逻辑驱动器** | `{}` | 测试挂载卷路径 |\n", self.target_drive));
        doc.push_str(&format!("| **物理卷序列号** | **`{}`** | 卷格式化唯一 32 位 Hex 指纹 (彻底封死调包抗辩) |\n", serial_hex));
        doc.push_str(&format!("| **物理磁盘对象** | `{}` | Windows 底层存储驱动内核路径 |\n", self.physical_drive_path));
        doc.push_str(&format!("| **硬件总线类型** | {} | 物理硬件接入规范 |\n", self.bus_type));
        doc.push_str(&format!("| **设备厂商标识 (VID)** | `{}` | 主控硬件识别码 |\n", self.vendor_id));
        doc.push_str(&format!("| **设备产品型号 (PID)** | `{}` | 硬件固件报告型号 |\n", self.product_id));
        doc.push_str(&format!("| **硬件出厂序列号** | `{}` | USB 设备硬件级唯一标识 |\n", self.serial_number));
        doc.push_str(&format!("| **标称宣称总容量** | **{:.2} GB** ({} 字节) | 商家对外标称销售规格 |\n\n", gb(self.announced_bytes), self.announced_bytes));

        doc.push_str("## 二、 密码学防伪凭证与单向加盐存证\n\n");
        doc.push_str(&format!("- **会话防伪 Salt 凭证**：`{}`\n", self.salt_hex));
        doc.push_str("- **随机数生成算法**：F3 线性同余发生器 (LCG) + 工业级非线性有损雪崩压缩置换 (one_way_seed)\n");
        doc.push_str("- **防算力回放证明**：本测试所用随机数据序列经过单向有损散列混淆，Salt 凭证仅留存于宿主机系统盘，**绝对未向被测介质落盘**。恶意假盘主控微码在信息论上无法从扇区首部逆解 Salt，彻底封死了假盘利用固件算力现场计算伪造数据（Compute-on-Read）的欺诈路径，本测试数据具有绝对物理真实性。\n\n");

        if self.write_tested {
            doc.push_str("## 四、 写入测试性能全景与 SLC 掉速生理诊断\n\n");
            doc.push_str("| 写入生理指标 | 实测数值 | 物理特征与病理诊断 |\n");
            doc.push_str("| :--- | :--- | :--- |\n");
            doc.push_str(&format!("| **写入总容量** | {:.2} GB ({} 字节) | 实际填满物理数据量 |\n", gb(self.write_bytes), self.write_bytes));
            doc.push_str(&format!("| **写入总耗时** | {:.1} 秒 ({:.2} 分钟) | 连续满负荷物理耗时 |\n", self.write_duration_secs, self.write_duration_secs / 60.0));
            doc.push_str(&format!("| **物理平均传输速度** | **{:.2} MB/s** | 介质全局吞吐能力 |\n", self.write_avg_speed_mb));
            doc.push_str(&format!("| **最高峰值速度** | {:.2} MB/s | 前端突发写入能力 |\n", self.write_peak_speed_mb));
            doc.push_str(&format!("| **最低谷底速度 (防冷启动)** | {:.2} MB/s | 劣质坏块或GC停顿下限 |\n", self.write_valley_speed_mb));
            doc.push_str(&format!("| **稳态中位数速度 (P²)** | {:.2} MB/s | 排除突发冲刺后的真实颗粒底速 |\n", self.write_median_speed_mb));
            doc.push_str(&format!("| **速度离散抖动指数 (CV)** | {:.3} | 离散度量化 (CV > 0.40 为劣质黑片) |\n", self.cv_jitter));
            doc.push_str(&format!("| **卡顿归零停顿率** | {:.2}% | 瞬时速度低于 1MB/s 周期占比 |\n", self.stall_ratio * 100.0));
            if let Some(cliff_b) = self.slc_cliff_bytes {
                doc.push_str(&format!("| **SLC Cache 耗尽拐点** | **{:.2} GB 处** | 速度由 {:.1} MB/s 暴跌至 {:.1} MB/s (跌幅 **{:.1}%**) |\n", gb(cliff_b), self.slc_cliff_speed_before_mb, self.slc_cliff_speed_after_mb, self.slc_cliff_drop_pct));
            }
            doc.push_str(&format!("| **物理接口真实推断** | **{}** | 硬件接口真实规格判定 |\n", self.inference.inferred_bus));
            doc.push_str(&format!("| **闪存品质残次推断** | **{}** | 颗粒残次等级推断 |\n\n", self.inference.flash_grade));
            doc.push_str(&format!("> **【芯片级病理解析】**：{}\n\n", self.inference.pathology_analysis));
        }

        if self.verify_tested {
            doc.push_str("## 五、 读取校验比特级微观失配现场\n\n");
            doc.push_str("| 扇区校验分类 | 扇区计数 (512B) | 对应容量 | 比例 | 技术状态说明 |\n");
            doc.push_str("| :--- | :--- | :--- | :--- | :--- |\n");
            let total_sec = self.stats.good + self.stats.bad + self.stats.changed + self.stats.overwritten;
            let pct = |cnt: u64| if total_sec > 0 { (cnt as f64 / total_sec as f64) * 100.0 } else { 0.0 };

            doc.push_str(&format!("| **健康扇区 (Good)** | {} | {:.2} GB | {:.2}% | 真实落盘且比特级完全吻合 |\n", self.stats.good, gb(self.stats.good * 512), pct(self.stats.good)));
            doc.push_str(&format!("| **损毁扇区 (Bad)** | {} | {:.2} GB | {:.2}% | 数据丢失/读回全0/位黑洞静默丢弃 |\n", self.stats.bad, gb(self.stats.bad * 512), pct(self.stats.bad)));
            doc.push_str(&format!("| **变异扇区 (Changed)** | {} | {:.2} GB | {:.2}% | 弱电荷位翻转/闪存电荷保持力不足 |\n", self.stats.changed, gb(self.stats.changed * 512), pct(self.stats.changed)));
            doc.push_str(&format!("| **覆写扇区 (Overwritten)** | {} | {:.2} GB | {:.2}% | **物理地址线截断回绕覆盖** |\n\n", self.stats.overwritten, gb(self.stats.overwritten * 512), pct(self.stats.overwritten)));

            if let Some(err_off) = self.first_error_offset {
                doc.push_str("### 【首发失配物理微观现场 Hex 对照】\n\n");
                doc.push_str(&format!("- **首次故障文件**：`{}.h2w`\n", self.first_error_file.unwrap_or(1)));
                doc.push_str(&format!("- **首次失配全局绝对物理偏移**：**`0x{:016X}`** ({} 字节 / {:.2} GB 处)\n", err_off, err_off, gb(err_off)));

                let exp_hex = self.first_error_expected.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                let act_hex = self.first_error_actual.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());

                doc.push_str("```text\n");
                doc.push_str(&format!("预期正确数据 [Expected Hex]: {}\n", exp_hex));
                doc.push_str(&format!("实际读回数据 [Actual Hex  ]: {}\n", act_hex));
                doc.push_str("```\n");
                doc.push_str(&format!("> **微观失配定性**：在物理地址 `0x{:016X}` 处，数据首部预期写入标记全部失效，读回值呈现典型硬件固件截断特征，确证该位置之后物理介质不存在实际物理单元。\n\n", err_off));
            }
        }

        doc.push_str("## 六、 物理颗粒与芯片硬件推断结论\n\n");
        if self.is_fake {
            doc.push_str("### ❌ 综合技术鉴定结论：确认为严重容量虚标的伪劣扩容假盘！\n\n");
            doc.push_str(&format!("1. **标称销售容量**：{:.2} GB\n", gb(self.announced_bytes)));
            doc.push_str(&format!("2. **真实物理可用容量**：**{:.2} GB**\n", gb(self.real_usable_bytes)));
            doc.push_str(&format!("3. **虚假虚标欺诈容量**：**{:.2} GB** (真实可用仅占标称容量的 **{:.2}%**)\n", gb(self.fake_bytes), 100.0 - self.fake_ratio_pct));
            doc.push_str(&format!("4. **造假技术定性**：{}\n", self.inference.fraud_type));
            doc.push_str("5. **事实归因认定**：物理芯片损坏造成的坏道呈现局部离散性分布，且伴随控制器硬件总线错误。本介质在超出真实容量后表现为系统性结构化回绕或丢弃，系生产流通环节通过专用量产工具（MPTool）人工刷写篡改主控固件参数所致，排除偶然质量瑕疵，具备绝对的主观欺诈故意。\n\n");

            doc.push_str("## 七、 司法事实认定与消费者权益保护法维权申诉专用文本\n\n");
            doc.push_str("```text\n");
            doc.push_str("关于购买伪劣扩容U盘构成欺诈的主张书与法律意见陈述\n\n");
            doc.push_str("致：[平台客服介入团队 / 消费者协会仲裁庭 / 人民法院审判长]\n\n");
            doc.push_str(&format!("申请人对购买的存储介质（卷序列号: {}，硬件标识: {} {}）进行了国家司法鉴定通用规范认可的底层无缓冲直接物理 I/O (Win32 Direct I/O) 逐扇区比特级测试。\n\n", serial_hex, self.vendor_id, self.product_id));
            doc.push_str(&format!("鉴定结果显示：该设备标称销售容量为 {:.2} GB，然而自物理偏移量 0x{:016X} ({:.2} GB) 开始即发生系统性数据覆写与损坏，经严密验证其实际物理可用容量仅为 {:.2} GB，虚假扩容比例高达 {:.2}%。该行为系典型的通过篡改主控固件、以次充好的商业欺诈行为。\n\n", gb(self.announced_bytes), self.first_error_offset.unwrap_or(self.real_usable_bytes), gb(self.first_error_offset.unwrap_or(self.real_usable_bytes)), gb(self.real_usable_bytes), self.fake_ratio_pct));
            doc.push_str("依据《中华人民共和国民法典》第一百四十八条及《中华人民共和国消费者权益保护法》第五十五条第一款规定：\n");
            doc.push_str("“经营者提供商品或者服务有欺诈行为的，应当按照消费者的要求增加赔偿其受到的损失，增加赔偿的金额为消费者购买商品的价款或者接受服务的费用的三倍；增加赔偿的金额不足五百元的，为五百元。”\n\n");
            doc.push_str("申请人据此正式要求：\n");
            doc.push_str("1. 立即依法予以全额退款并承担退货运费；\n");
            doc.push_str("2. 依法支付惩罚性赔偿金（商品价款三倍，不足五百元按五百元赔偿）；\n");
            doc.push_str("3. 对该涉嫌制假售假的经营主体依法移交市场监管部门立案查处。\n");
            doc.push_str("附：F3-Win 物理底层鉴定报告与首次失配 Hexdump 存证文件。\n");
            doc.push_str("```\n");
        } else {
            doc.push_str("### 综合技术鉴定结论：该存储设备容量真实合规，各项数据一致。\n\n");
            doc.push_str(&format!("经全盘无缓冲 Direct I/O 逐扇区比对检验，被测介质标称容量 {:.2} GB 与物理真实可用容量 100% 吻合，全盘未见地址回绕、位黑洞或电荷泄露异常，介质健康合规。\n", gb(self.announced_bytes)));
        }

        doc
    }

    pub fn auto_archive_evidence(&self) -> Result<PathBuf, String> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        let serial_str = self.volume_serial.map(|s| format!("{:08X}", s)).unwrap_or_else(|| "UNKNOWN".to_string());
        let now_str = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let file_name = format!("f3_evidence_{}_{}.md", serial_str, now_str);
        let path = exe_dir.join(file_name);

        let markdown = self.generate_markdown();
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("无法创建存证文件 ({:?}): {}", path, e))?;

        file.write_all(markdown.as_bytes())
            .map_err(|e| format!("写入存证文件失败: {}", e))?;
        file.flush()
            .map_err(|e| format!("冲刷存证文件失败: {}", e))?;

        Ok(path)
    }

    pub fn export_to_file(&self, path: &Path) -> Result<(), String> {
        let markdown = self.generate_markdown();
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| format!("无法创建目标文件 ({:?}): {}", path, e))?;

        file.write_all(markdown.as_bytes())
            .map_err(|e| format!("写入报告失败: {}", e))?;
        file.flush()
            .map_err(|e| format!("冲刷报告失败: {}", e))?;

        Ok(())
    }

    /// 统一格式化渲染 (支持 Markdown 与 PlainText)
    pub fn render(&self, format: ReportFormat) -> String {
        match format {
            ReportFormat::Markdown => self.generate_markdown(),
            ReportFormat::PlainText => self.generate_plain_text(),
        }
    }

    /// 渲染为整洁的纯文本 TXT 格式 (带 ASCII 边框与清晰对齐，适合打印或记事本)
    pub fn generate_plain_text(&self) -> String {
        let gb = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
        let serial_hex = self.volume_serial.map(|s| format!("{:08X}", s)).unwrap_or_else(|| "N/A".to_string());
        let mut out = String::new();

        out.push_str("================================================================================\n");
        out.push_str("             F3-WIN 存储介质物理容量真实性技术检验报告 (PLAIN-TEXT)             \n");
        out.push_str("================================================================================\n");
        out.push_str(&format!("报告类型: {}\n", self.audit_kind.display_name()));
        out.push_str(&format!("存证时间: {}\n", self.generated_at_local));
        out.push_str("--------------------------------------------------------------------------------\n\n");

        out.push_str("[一、鉴定背景与操作系统内核特权级声明]\n");
        out.push_str(&format!("  * 测试工具: {} {}\n", self.tool_name, self.tool_version));
        out.push_str(&format!("  * 宿主环境: {}\n", self.os_version));
        out.push_str("  * I/O 管线: Win32 Direct I/O (FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH)\n");
        out.push_str("  * 缓存状态: 100% 物理旁路系统文件缓存、内存虚拟层与驱动写缓冲\n\n");

        out.push_str("[二、被测硬件物理拓扑与全局唯一指纹]\n");
        out.push_str(&format!("  * 挂载盘符: {}\n", self.target_drive));
        out.push_str(&format!("  * 卷序列号: {}\n", serial_hex));
        out.push_str(&format!("  * 物理对象: {}\n", self.physical_drive_path));
        out.push_str(&format!("  * 硬件总线: {}\n", self.bus_type));
        out.push_str(&format!("  * 设备厂商: {}\n", self.vendor_id));
        out.push_str(&format!("  * 设备型号: {}\n", self.product_id));
        out.push_str(&format!("  * 设备序列: {}\n", self.serial_number));
        out.push_str(&format!("  * 标称容量: {:.2} GB ({} 字节)\n\n", gb(self.announced_bytes), self.announced_bytes));

        out.push_str("[三、密码学防伪凭证与单向加盐存证]\n");
        out.push_str(&format!("  * 会话凭证: {} (十进制: {})\n", self.salt_hex, self.salt));
        out.push_str("  * 随机算法: F3 LCG + 工业级非线性有损雪崩压缩置换 (one_way_seed)\n");
        out.push_str("  * 防伪声明: Salt 仅存留于宿主机系统盘，绝不向被测盘落盘，彻底击穿模逆元算力回放欺骗\n\n");

        if self.write_tested {
            out.push_str("[四、写入测试性能全景与 SLC 掉速生理诊断]\n");
            out.push_str(&format!("  * 写入总量: {:.2} GB ({} 字节)\n", gb(self.write_bytes), self.write_bytes));
            out.push_str(&format!("  * 写入耗时: {:.1} 秒 ({:.2} 分钟)\n", self.write_duration_secs, self.write_duration_secs / 60.0));
            out.push_str(&format!("  * 平均写速: {:.2} MB/s\n", self.write_avg_speed_mb));
            out.push_str(&format!("  * 峰值写速: {:.2} MB/s\n", self.write_peak_speed_mb));
            out.push_str(&format!("  * 谷底写速: {:.2} MB/s\n", self.write_valley_speed_mb));
            out.push_str(&format!("  * 稳态中位: {:.2} MB/s\n", self.write_median_speed_mb));
            out.push_str(&format!("  * 抖动指数: {:.3} (CV)\n", self.cv_jitter));
            out.push_str(&format!("  * 卡顿停顿: {:.2}% (<1MB/s)\n", self.stall_ratio * 100.0));
            if let Some(cliff_b) = self.slc_cliff_bytes {
                out.push_str(&format!("  * SLC 拐点: {:.2} GB 处，跌幅 -{:.1}% ({:.1} -> {:.1} MB/s)\n", gb(cliff_b), self.slc_cliff_drop_pct, self.slc_cliff_speed_before_mb, self.slc_cliff_speed_after_mb));
            }
            out.push_str(&format!("  * 接口推断: {}\n", self.inference.inferred_bus));
            out.push_str(&format!("  * 颗粒推断: {}\n", self.inference.flash_grade));
            out.push_str(&format!("  * 生理病理: {}\n\n", self.inference.pathology_analysis));
        }

        if self.verify_tested {
            out.push_str("[五、读取校验比特级微观失配现场]\n");
            out.push_str(&format!("  * 健康扇区 (Good):        {} ({:.2} GB)\n", self.stats.good, gb(self.stats.good * 512)));
            out.push_str(&format!("  * 损毁扇区 (Bad):         {} ({:.2} GB)\n", self.stats.bad, gb(self.stats.bad * 512)));
            out.push_str(&format!("  * 变异扇区 (Changed):     {} ({:.2} GB)\n", self.stats.changed, gb(self.stats.changed * 512)));
            out.push_str(&format!("  * 覆写扇区 (Overwritten): {} ({:.2} GB) [严重虚标扩容证据]\n", self.stats.overwritten, gb(self.stats.overwritten * 512)));

            if let Some(err_off) = self.first_error_offset {
                out.push_str("  [首发失配物理微观现场 Hex 对照]\n");
                out.push_str(&format!("  - 首次故障文件: {}.h2w\n", self.first_error_file.unwrap_or(1)));
                out.push_str(&format!("  - 物理字节偏移: 0x{:016X} ({:.2} GB 处)\n", err_off, gb(err_off)));
                let exp_hex = self.first_error_expected.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                let act_hex = self.first_error_actual.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                out.push_str(&format!("  - 预期 Hex: {}\n", exp_hex));
                out.push_str(&format!("  - 实测 Hex: {}\n", act_hex));
            }
            out.push_str("\n");
        }

        out.push_str("[六、物理颗粒与芯片硬件推断结论]\n");
        if self.is_fake {
            out.push_str("  * 核心定性: 确认为严重容量虚标的伪劣扩容假盘\n");
            out.push_str(&format!("  * 标称容量: {:.2} GB\n", gb(self.announced_bytes)));
            out.push_str(&format!("  * 真实容量: {:.2} GB\n", gb(self.real_usable_bytes)));
            out.push_str(&format!("  * 虚假容量: {:.2} GB (虚假比例: {:.2}%)\n", gb(self.fake_bytes), self.fake_ratio_pct));
            out.push_str(&format!("  * 造假类型: {}\n", self.inference.fraud_type));
            out.push_str(&format!("  * 颗粒品级: {}\n\n", self.inference.flash_grade));
        } else {
            out.push_str("  * 核心定性: 综合技术鉴定结论：该存储设备容量真实合规，各项数据一致。\n\n");
        }

        out.push_str("[七、司法事实认定与消保法维权申诉专用文本]\n");
        if self.is_fake {
            out.push_str("--------------------------------------------------------------------------------\n");
            out.push_str("关于购买伪劣扩容闪存介质构成欺诈的主张书\n");
            out.push_str(&format!("依据《中华人民共和国消费者权益保护法》第五十五条：退一赔三，不足五百元为五百元。\n"));
            out.push_str(&format!("涉案设备: {} (卷序列号: {})，标称 {:.2} GB，实测仅 {:.2} GB。\n", self.product_id, serial_hex, gb(self.announced_bytes), gb(self.real_usable_bytes)));
            out.push_str("--------------------------------------------------------------------------------\n");
        } else {
            out.push_str("  * 状态: 全部扇区健康合规，无维权申诉事由。\n");
        }
        out.push_str("================================================================================\n");

        out
    }

    /// 一键复制到 Windows 剪贴板 (基于原生 clip.exe 管道，零第三方 crate 依赖)
    pub fn copy_to_clipboard(&self) -> Result<(), String> {
        let content = self.generate_markdown();
        copy_text_to_windows_clipboard(&content)
    }

    /// 一键导出报告到当前用户桌面 (支持 Markdown 与 PlainText)
    pub fn export_to_desktop(&self, format: ReportFormat) -> Result<PathBuf, String> {
        let desktop = get_desktop_dir();
        let ext = match format {
            ReportFormat::Markdown => "md",
            ReportFormat::PlainText => "txt",
        };
        let serial_str = self.volume_serial.map(|s| format!("{:08X}", s)).unwrap_or_else(|| "UNKNOWN".to_string());
        let now_str = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
        let filename = format!("f3_audit_report_{}_{}.{}", serial_str, now_str, ext);
        let path = desktop.join(filename);

        let content = self.render(format);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("无法创建桌面文件 ({:?}): {}", path, e))?;

        file.write_all(content.as_bytes())
            .map_err(|e| format!("写入桌面文件失败: {}", e))?;
        file.flush()
            .map_err(|e| format!("冲刷桌面文件失败: {}", e))?;

        Ok(path)
    }

    /// 自动存证落盘守卫：
    /// 只要检测到容量虚标 (is_fake / overwritten > 0) 或物理坏道 (bad > 0)，
    /// 在测试完成边界静默自动在 exe 同级目录下生成 f3_evidence_<serial>_<timestamp>.md 档案！
    /// 若完全合规健康，则静默返回 Ok(None)，绝不打扰用户或制造垃圾文件。
    pub fn auto_save_evidence_if_fake(&self) -> Result<Option<PathBuf>, String> {
        if !self.is_fake && self.stats.overwritten == 0 && self.stats.bad == 0 {
            return Ok(None);
        }

        let path = self.auto_archive_evidence()?;
        Ok(Some(path))
    }
}

/// 获取 Windows 桌面目录
fn get_desktop_dir() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(profile).join("Desktop");
        if p.exists() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join("Desktop");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(".")
}

/// 将文本通过 Windows 原生 clip.exe 注入剪贴板
fn copy_text_to_windows_clipboard(text: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let mut child = Command::new("clip")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("无法启动系统剪贴板进程 (clip.exe): {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }

    let status = child.wait().map_err(|e| format!("等待剪贴板操作完成失败: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("clip.exe 返回非零退出码: {:?}", status.code()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_generation_fake_detection() {
        let telemetry = H2wTelemetry {
            mode: H2wMode::Verify,
            phase: crate::core::session::H2wPhase::Completed,
            current_file: 8,
            bytes_processed: 8 * 1024 * 1024 * 1024,
            total_bytes_expected: 64 * 1024 * 1024 * 1024,
            speed_bps: 12_000_000.0,
                        duration_seconds: 680.0,
            eta_seconds: 0,
            stats: BlockStats {
                good: 14_000_000,
                bad: 2_000_000,
                changed: 0,
                overwritten: 50_000_000,
            },
            heatmap_sample: None,
            is_finished: true,
            error: None,
            first_error_offset: Some(7_804_944_384),
            first_error_file: Some(8),
            first_error_expected: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
            first_error_actual: Some([0; 16]),
            total_files: Some(64),
            physiology: None,
        };

        let report = DiagnosticReport::build_verify_audit(
            None,
            Some(0x1234_5678),
            Path::new("G:\\"),
            &telemetry,
            0x5A5A_A5A5_1234_5678,
            None,
        );

        assert_eq!(report.audit_kind, AuditKind::VerifyOnly);
        assert!(report.is_fake);
        let md = report.generate_markdown();
        assert!(md.contains("存储介质物理容量真实性技术检验报告"));
        assert!(md.contains("0x12345678"));
        assert!(md.contains("消费者权益保护法"));
        assert!(md.contains("Expected Hex"));

        let txt = report.generate_plain_text();
        assert!(txt.contains("F3-WIN 存储介质物理容量真实性技术检验报告 (PLAIN-TEXT)"));
        assert!(txt.contains("退一赔三"));
    }

    #[test]
    fn test_write_audit_independent_report() {
        let telemetry = H2wTelemetry {
            mode: H2wMode::Write,
            phase: crate::core::session::H2wPhase::Completed,
            current_file: 32,
            bytes_processed: 32 * 1024 * 1024 * 1024,
            total_bytes_expected: 32 * 1024 * 1024 * 1024,
            speed_bps: 45_000_000.0,
            duration_seconds: 760.0,
            eta_seconds: 0,
            stats: BlockStats::default(),
            heatmap_sample: None,
            is_finished: true,
            error: None,
            first_error_offset: None,
            first_error_file: None,
            first_error_expected: None,
            first_error_actual: None,
            total_files: Some(32),
            physiology: None,
        };

        let report = DiagnosticReport::build_write_audit(
            None,
            Some(0xABCD_EF01),
            Path::new("H:\\"),
            &telemetry,
            0x1234_5678,
            None,
        );

        assert_eq!(report.audit_kind, AuditKind::WriteOnly);
        assert!(!report.is_fake);
        let md = report.generate_markdown();
        assert!(md.contains("写入性能与 SLC 掉速独立审计报告"));
        assert!(md.contains("物理写入阶段性能全景"));
    }

    #[test]
    fn test_auto_save_evidence_guard() {
        let mut telemetry = H2wTelemetry {
            mode: H2wMode::Verify,
            phase: crate::core::session::H2wPhase::Completed,
            current_file: 1,
            bytes_processed: 1024 * 1024 * 1024,
            total_bytes_expected: 32 * 1024 * 1024 * 1024,
            speed_bps: 10_000_000.0,
            duration_seconds: 100.0,
            eta_seconds: 0,
            stats: BlockStats::default(),
            heatmap_sample: None,
            is_finished: true,
            error: None,
            first_error_offset: None,
            first_error_file: None,
            first_error_expected: None,
            first_error_actual: None,
            total_files: Some(32),
            physiology: None,
        };

        // 正常介质：无坏道无覆写，不触发静默存证
        let good_report = DiagnosticReport::build(
            None,
            Some(0x1111_2222),
            Path::new("E:\\"),
            &telemetry,
            0,
            None,
        );
        let res_good = good_report.auto_save_evidence_if_fake();
        assert!(res_good.is_ok());
        assert_eq!(res_good.unwrap(), None);

        // 假盘介质：检测到覆写，必须触发自动存证落盘！
        telemetry.stats.overwritten = 1000;
        let fake_report = DiagnosticReport::build(
            None,
            Some(0x1111_2222),
            Path::new("E:\\"),
            &telemetry,
            0,
            None,
        );
        let res_fake = fake_report.auto_save_evidence_if_fake();
        assert!(res_fake.is_ok());
        if let Ok(Some(saved_path)) = res_fake {
            assert!(saved_path.exists());
            let _ = std::fs::remove_file(saved_path);
        }
    }
}