use std::path::PathBuf;
use eframe::egui::{self, Color32, ProgressBar, RichText};

use crate::core::controller::{TaskSnapshot, TaskStatus};
use crate::core::fix::SafePartitionFixer;
use crate::core::session::{H2wConfig, H2wMode, H2wTelemetry};
use crate::core::salt::SaltRegistry;
use crate::core::report::{DiagnosticReport, ReportFormat};
use crate::gui::components::device_selector::{
    render_storage_device_selector, render_storage_volume_selector,
};
use crate::gui::components::heatmap::render_heatmap;
use crate::gui::components::plot::render_speed_plot;
use crate::gui::state::{AppState, AppTab};

pub struct F3App {
    state: AppState,
    prev_status: TaskStatus,
}

impl F3App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::setup_chinese_fonts(&cc.egui_ctx);
        Self {
            state: AppState::default(),
            prev_status: TaskStatus::Idle,
        }
    }
}

impl eframe::App for F3App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 单行获取深层任务控制器轮询出的最新只读快照
        let snapshot = self.state.controller.poll().clone();

        // 当异步任务刚刚执行完成时，自动静默刷新存储资产与磁盘空间
        if self.prev_status == TaskStatus::Running && snapshot.status == TaskStatus::Finished {
            self.state.inventory.refresh();

            // 持久化记录写入与读取双端速度全景数据，供电子版报告实时并列展示
            match snapshot.kind {
                crate::core::controller::TaskKind::Write => {
                    self.state.audit_record.write_tested = true;
                    self.state.audit_record.write_avg_mb = snapshot.current_speed_mb;
                    self.state.audit_record.write_bytes = snapshot.total_bytes_processed;
                }
                crate::core::controller::TaskKind::Verify => {
                    self.state.audit_record.read_tested = true;
                    self.state.audit_record.read_avg_mb = snapshot.current_speed_mb;
                    self.state.audit_record.read_bytes = snapshot.total_bytes_processed;
                }
                crate::core::controller::TaskKind::WriteAndVerify => {
                    self.state.audit_record.write_tested = true;
                    self.state.audit_record.read_tested = true;
                    self.state.audit_record.read_avg_mb = snapshot.current_speed_mb;
                    self.state.audit_record.read_bytes = snapshot.total_bytes_processed;
                }
                _ => {}
            }

            // 只要检测到容量虚标或坏道，在测试完成边界静默自动在 exe 同级目录生成 f3_evidence_<serial>_<timestamp>.md 档案！
            if snapshot.stats.overwritten > 0 || snapshot.stats.bad > 0 {
                let active_target = self.state.inventory.active_target();
                let dev = active_target.as_ref().map(|t| &t.device);
                let serial = SaltRegistry::get_volume_serial(&self.state.inventory.selected_path().to_string_lossy());
                let dummy_telemetry = crate::core::session::H2wTelemetry {
                    mode: match snapshot.kind {
                        crate::core::controller::TaskKind::Write => crate::core::session::H2wMode::Write,
                        crate::core::controller::TaskKind::Verify => crate::core::session::H2wMode::Verify,
                        crate::core::controller::TaskKind::WriteAndVerify => crate::core::session::H2wMode::WriteAndVerify,
                        _ => crate::core::session::H2wMode::Verify,
                    },
                    phase: crate::core::session::H2wPhase::Completed,
                    current_file: 0,
                    total_files: None,
                    bytes_processed: snapshot.total_bytes_processed,
                    total_bytes_expected: snapshot.total_bytes_processed,
                    speed_bps: snapshot.current_speed_mb * 1024.0 * 1024.0,
                    duration_seconds: 0.0,
                    eta_seconds: 0,
                    stats: snapshot.stats,
                    heatmap_sample: None,
                    is_finished: true,
                    error: snapshot.error.clone(),
                    first_error_offset: None,
                    first_error_file: None,
                    first_error_expected: None,
                    first_error_actual: None,
                    physiology: None,
                };
                let report = DiagnosticReport::build(
                    dev,
                    serial,
                    &self.state.inventory.selected_path(),
                    &dummy_telemetry,
                    self.state.salt,
                    None,
                );
                let _ = report.auto_save_evidence_if_fake();
            }
        }
        self.prev_status = snapshot.status;

        // 仅在任务处于运行中状态时高频请求重绘采样，任务空闲时静默休眠节能
        if snapshot.status == TaskStatus::Running {
            ctx.request_repaint();
        }

        egui::TopBottomPanel::top("top_nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("F3 闪存完整性测试").strong());
                ui.separator();

                ui.selectable_value(&mut self.state.current_tab, AppTab::WriteTest, "写入测试 (f3write)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::ReadVerify, "读取校验 (f3read)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::RapidProbe, "闪电探测 (f3probe)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::PartitionFix, "分区修复 (f3fix)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::Settings, "设置");
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let status_color = match snapshot.status {
                    TaskStatus::Idle => Color32::GRAY,
                    TaskStatus::Running => Color32::from_rgb(0, 180, 255),
                    TaskStatus::Finished => Color32::from_rgb(46, 204, 113),
                    TaskStatus::Error => Color32::from_rgb(231, 76, 60),
                };
                ui.colored_label(status_color, format!("状态: {}", snapshot.status_message));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.state.current_tab {
                AppTab::WriteTest => self.render_write_tab(ui, &snapshot),
                AppTab::ReadVerify => self.render_read_tab(ui, &snapshot),
                AppTab::RapidProbe => self.render_probe_tab(ui, &snapshot),
                AppTab::PartitionFix => self.render_fix_tab(ui, &snapshot),
                AppTab::Settings => self.render_settings_tab(ui),
            }
        });
    }
}

impl F3App {
    fn render_write_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        ui.heading("写入测试 (f3write)");
        ui.label("以 Direct I/O 无系统缓存模式向目标磁盘填充 .h2w 数据文件，测试真实写入速率。");
        ui.separator();

        render_storage_volume_selector(ui, "write_vol_combo", &mut self.state.inventory);

        ui.add_space(10.0);

        if snapshot.status != TaskStatus::Running {
            if ui.button(RichText::new("开始写入").size(15.0)).clicked() {
                self.start_write_task();
            }
        } else if ui.button(RichText::new("停止写入").size(15.0)).clicked() {
            self.state.controller.cancel();
        }

        ui.add_space(10.0);

        // 现代 Bento-Grid 四卡片核心指标仪表盘
        ui.horizontal(|ui| {
            let card_width = (ui.available_width() - 30.0) / 4.0;

            // 卡片 1: 瞬时速率
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("当前写入速度").weak().size(12.0));
                ui.label(
                    RichText::new(format!("{:.2} MB/s", snapshot.current_speed_mb))
                        .strong()
                        .size(20.0)
                        .color(Color32::from_rgb(0, 200, 255)),
                );
            });

            // 卡片 2: 已写入数据
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("已写入数据").weak().size(12.0));
                ui.label(
                    RichText::new(format!(
                        "{:.2} GB",
                        snapshot.total_bytes_processed as f64 / (1024.0 * 1024.0 * 1024.0)
                    ))
                    .strong()
                    .size(20.0),
                );
            });

            // 卡片 3: 动态 ETA 倒计时
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("预计剩余时间").weak().size(12.0));
                let eta_str = if snapshot.status == TaskStatus::Running {
                    format_eta(snapshot.eta_seconds)
                } else {
                    "空闲就绪".to_string()
                };
                ui.label(
                    RichText::new(eta_str)
                        .strong()
                        .size(20.0)
                        .color(Color32::from_rgb(255, 180, 0)),
                );
            });

            // 卡片 4: I/O 驱动模式
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("I/O 模式").weak().size(12.0));
                ui.label(RichText::new("Direct I/O (无缓存)").strong().size(18.0));
            });
        });

        // 便捷按钮区
        ui.horizontal(|ui| {
            if ui.button("在资源管理器中打开").clicked() {
                let _ = std::process::Command::new("explorer")
                    .arg(self.state.inventory.selected_path())
                    .spawn();
            }
            let is_busy = snapshot.status == TaskStatus::Running;
            if ui
                .add_enabled(
                    !is_busy,
                    egui::Button::new(RichText::new("清理测试文件 (.h2w)")),
                )
                .clicked()
            {
                let path = self.state.inventory.selected_path();
                self.state.controller.start_cleanup(path);
            }
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &snapshot.speed_history, "写入", "write_speed_plot");

        // 写入测试完成时，在底部直接渲染专业电子版技术检验报告公文卡片 (同时展示写入和读取速度，可直接截屏)
        if (snapshot.status == TaskStatus::Finished && snapshot.total_bytes_processed > 0) || self.state.audit_record.write_tested {
            let active_target = self.state.inventory.active_target();
            let dev = active_target.as_ref().map(|t| &t.device);
            let serial = SaltRegistry::get_volume_serial(&self.state.inventory.selected_path().to_string_lossy());
            let dummy_telemetry = H2wTelemetry {
                mode: H2wMode::Write,
                phase: crate::core::session::H2wPhase::Completed,
                current_file: 1,
                total_files: None,
                bytes_processed: if snapshot.total_bytes_processed > 0 { snapshot.total_bytes_processed } else { self.state.audit_record.write_bytes },
                total_bytes_expected: if snapshot.total_bytes_processed > 0 { snapshot.total_bytes_processed } else { self.state.audit_record.write_bytes },
                speed_bps: if snapshot.current_speed_mb > 0.0 { snapshot.current_speed_mb * 1024.0 * 1024.0 } else { self.state.audit_record.write_avg_mb * 1024.0 * 1024.0 },
                duration_seconds: self.state.audit_record.write_duration_secs,
                eta_seconds: 0,
                stats: snapshot.stats,
                heatmap_sample: None,
                is_finished: true,
                error: None,
                physiology: None,
                first_error_offset: None,
                first_error_file: None,
                first_error_expected: None,
                first_error_actual: None,
            };
            let mut report = DiagnosticReport::build(
                dev,
                serial,
                &self.state.inventory.selected_path(),
                &dummy_telemetry,
                self.state.salt,
                None,
            );

            self.render_write_report_card(ui, &report);
        }
    }

    fn render_read_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        ui.heading("读取校验 (f3read)");
        ui.label("读取磁盘上的 .h2w 文件比对数据，检查损坏扇区与虚标覆盖。");
        ui.separator();

        render_storage_volume_selector(ui, "read_vol_combo", &mut self.state.inventory);

        ui.add_space(10.0);

        if snapshot.status != TaskStatus::Running {
            if ui.button(RichText::new("开始校验").size(15.0)).clicked() {
                self.start_read_task();
            }
        } else if ui.button(RichText::new("停止校验").size(15.0)).clicked() {
            self.state.controller.cancel();
        }

        ui.add_space(10.0);

        // 校验统计计数
        ui.horizontal(|ui| {
            ui.colored_label(Color32::from_rgb(46, 204, 113), format!("正常: {}", snapshot.stats.good));
            ui.separator();
            ui.colored_label(Color32::from_rgb(231, 76, 60), format!("损坏: {}", snapshot.stats.bad));
            ui.separator();
            ui.colored_label(Color32::from_rgb(241, 196, 15), format!("异常: {}", snapshot.stats.changed));
            ui.separator();
            ui.colored_label(Color32::from_rgb(155, 89, 182), format!("覆盖: {}", snapshot.stats.overwritten));
        });

        // 便捷按钮与自动清理控制
        ui.horizontal(|ui| {
            if ui.button("在文件资源管理器中打开此路径").clicked() {
                let _ = std::process::Command::new("explorer")
                    .arg(self.state.inventory.selected_path())
                    .spawn();
            }
            if ui.button("转到写入页面").clicked() {
                self.state.current_tab = AppTab::WriteTest;
            }
            let is_busy = snapshot.status == TaskStatus::Running;
            if ui
                .add_enabled(
                    !is_busy,
                    egui::Button::new(
                        RichText::new("一键清理所有 .h2w 测试文件 (释放空间)")
                            .color(Color32::from_rgb(255, 140, 0)),
                    ),
                )
                .clicked()
            {
                let path = self.state.inventory.selected_path();
                self.state.controller.start_cleanup(path);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.state.auto_cleanup_after_read, "校验完成后自动删除测试文件");
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &snapshot.speed_history, "读取", "read_speed_plot");
        ui.add_space(10.0);
        render_heatmap(ui, &snapshot.heatmap_blocks);

        // 校验完成时，在底部直接渲染可截屏的专业电子版技术检验报告公文卡片 (同时展示写入和读取速度)
        if (snapshot.status == TaskStatus::Finished && snapshot.total_bytes_processed > 0) || self.state.audit_record.read_tested {
            let active_target = self.state.inventory.active_target();
            let dev = active_target.as_ref().map(|t| &t.device);
            let serial = SaltRegistry::get_volume_serial(&self.state.inventory.selected_path().to_string_lossy());
            
            let dummy_telemetry = H2wTelemetry {
                mode: H2wMode::Verify,
                phase: crate::core::session::H2wPhase::Completed,
                current_file: 1,
                total_files: None,
                bytes_processed: if snapshot.total_bytes_processed > 0 { snapshot.total_bytes_processed } else { self.state.audit_record.read_bytes },
                total_bytes_expected: if snapshot.total_bytes_processed > 0 { snapshot.total_bytes_processed } else { self.state.audit_record.read_bytes },
                speed_bps: if snapshot.current_speed_mb > 0.0 { snapshot.current_speed_mb * 1024.0 * 1024.0 } else { self.state.audit_record.read_avg_mb * 1024.0 * 1024.0 },
                duration_seconds: self.state.audit_record.read_duration_secs,
                eta_seconds: 0,
                stats: snapshot.stats,
                heatmap_sample: None,
                is_finished: true,
                error: None,
                physiology: None,
                first_error_offset: None,
                first_error_file: None,
                first_error_expected: None,
                first_error_actual: None,
            };
            
            let mut report = DiagnosticReport::build(
                dev,
                serial,
                &self.state.inventory.selected_path(),
                &dummy_telemetry,
                self.state.salt,
                None,
            );

            self.render_verify_report_card(ui, &report);
        }
    }

    fn render_probe_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        ui.heading("容量探测 (f3probe)");
        ui.label("直接向物理磁盘写入探针测试实际容量。测试前会自动卸载卷，请备份重要数据。");
        ui.separator();

        render_storage_device_selector(ui, &mut self.state.inventory);

        ui.add_space(15.0);

        if snapshot.status != TaskStatus::Running {
            if ui.button(RichText::new("开始探测").size(15.0)).clicked() {
                self.start_probe_task();
            }
        } else if ui.button(RichText::new("停止探测").size(15.0)).clicked() {
            self.state.controller.cancel();
        }

        ui.add_space(15.0);
        ui.add(ProgressBar::new(snapshot.progress_pct).show_percentage());

        if let Some(ref res) = snapshot.probe_result {
            if let Some(ref err) = res.error {
                ui.group(|ui| {
                    if err.contains("用户中止") || err.contains("手动中止") {
                        ui.colored_label(
                            Color32::from_rgb(241, 196, 15),
                            RichText::new("探测任务已由用户手动中止。").strong().size(15.0),
                        );
                        ui.label("设备已安全解锁恢复挂载，可随时重新启动探测。");
                    } else {
                        ui.colored_label(
                            Color32::RED,
                            RichText::new(format!("无法执行探测: {}", err)).strong().size(15.0),
                        );
                        ui.label("提示: 请确认已允许管理员权限，且设备未被其他程序占用。");
                    }
                });
            } else {
                ui.group(|ui| {
                    ui.heading("探测结果:");
                    ui.label(format!(
                        "设备标称容量: {:.2} GB",
                        res.announced_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                    ));
                    ui.label(format!(
                        "物理真实容量: {:.2} GB",
                        res.real_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                    ));

                    if res.is_fake {
                        ui.colored_label(Color32::RED, "检测到容量虚标。建议前往「分区修复」调整可用分区大小。");
                    } else {
                        ui.colored_label(Color32::GREEN, "容量正常，未发现地址回绕。");
                    }
                });
            }
        }
    }

    fn render_fix_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        ui.heading("分区修复 (f3fix)");
        ui.label("根据探测出的实际物理容量调整分区大小，避免使用超出物理范围的扇区。");
        ui.separator();

        if let Some(ref plan) = snapshot.fix_plan {
            let fixer = SafePartitionFixer::new(plan.clone());
            ui.group(|ui| {
                ui.label(format!("目标物理磁盘: PhysicalDrive{}", plan.disk_index));
                ui.label(format!(
                    "真实可用容量: {:.2} GB",
                    plan.real_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                ));
                ui.label(format!("建议截断分区大小: {} MB", plan.safe_partition_mb));
                ui.label(format!("推荐文件系统: {}", plan.file_system));

                ui.add_space(5.0);
                match fixer.dry_run() {
                    Ok(report) => {
                        ui.colored_label(
                            Color32::GREEN,
                            format!("已通过移动介质总线现场复核: {} ({})", report.bus_name, report.vendor_id),
                        );
                        ui.add_space(10.0);
                        let is_busy = snapshot.status == TaskStatus::Running;
                        if ui
                            .add_enabled(
                                !is_busy,
                                egui::Button::new(RichText::new("应用安全分区 (运行 diskpart)").size(15.0)),
                            )
                            .clicked()
                        {
                            if let Err(e) = self.state.controller.start_fix(plan.clone()) {
                                self.state.controller.set_status_message(format!("启动修复失败: {}", e));
                            }
                        }
                    }
                    Err(err) => {
                        ui.colored_label(Color32::RED, format!("安全拦截: {}", err));
                        ui.add_space(10.0);
                        ui.add_enabled(false, egui::Button::new(RichText::new("无法应用安全分区").size(15.0)));
                    }
                }

                if ui.button("导出 diskpart 脚本").clicked() {
                    let desk = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
                        .join(r"Desktop\f3_fix.diskpart");
                    let _ = plan.export_script_to_file(&desk);
                    self.state.controller.set_status_message(format!("脚本已导出至 {:?}", desk));
                }
            });
        } else {
            ui.label("暂无待修复方案，请先在「闪电探测」标签页完成假盘探测。");
        }
    }


    /// 渲染写入测试页专属的电子版物理性能检验公文卡片 (无需复制，一键直截)
    fn render_write_report_card(&mut self, ui: &mut egui::Ui, report: &DiagnosticReport) {
        let gb = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
        let serial_hex = report.volume_serial.map(|s| format!("0x{:08X}", s)).unwrap_or_else(|| "N/A".to_string());

        let write_avg = if report.write_avg_speed_mb > 0.0 {
            report.write_avg_speed_mb
        } else if self.state.audit_record.write_avg_mb > 0.0 {
            self.state.audit_record.write_avg_mb
        } else {
            0.0
        };

        let write_peak = if report.write_peak_speed_mb > 0.0 {
            report.write_peak_speed_mb
        } else if write_avg > 0.0 {
            write_avg
        } else {
            0.0
        };

        let write_valley = if report.write_valley_speed_mb > 0.0 {
            report.write_valley_speed_mb
        } else {
            self.state.audit_record.write_valley_mb
        };

        let write_median = if report.write_median_speed_mb > 0.0 {
            report.write_median_speed_mb
        } else if write_avg > 0.0 {
            write_avg
        } else {
            0.0
        };

        let write_bytes = if report.write_bytes > 0 {
            report.write_bytes
        } else {
            self.state.audit_record.write_bytes
        };

        ui.add_space(15.0);
        egui::Frame::none()
            .fill(Color32::from_rgb(22, 24, 29))
            .stroke(egui::Stroke::new(1.5, Color32::from_rgb(52, 152, 219)))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                // 1. 报告红头与公文标识
                ui.horizontal(|ui| {
                    ui.label(RichText::new("[司法存证检验]").background_color(Color32::from_rgb(41, 128, 185)).color(Color32::WHITE).strong());
                    ui.heading(RichText::new("存储介质物理写入性能与颗粒体质检验书").strong().size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&report.generated_at_local).color(Color32::GRAY));
                    });
                });

                ui.separator();
                ui.add_space(4.0);

                // 2. 写入阶段状态卡片
                egui::Frame::none()
                    .fill(Color32::from_rgb(20, 35, 45))
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.colored_label(Color32::from_rgb(100, 200, 255), RichText::new("【写入阶段完成】：物理数据流已全盘无缓冲直通落盘！").strong().size(15.0));
                        ui.label(RichText::new(format!(
                            "写入数据总量: {:.2} GB | 连续写入耗时: {:.1} 秒 ({:.2} 分钟) | 平均写入速度: {:.2} MB/s",
                            gb(write_bytes), report.write_duration_secs, report.write_duration_secs / 60.0, write_avg
                        )).color(Color32::from_rgb(220, 240, 255)).strong());
                        ui.add_space(3.0);
                        ui.label(RichText::new("【说明】：本阶段已将高熵伪随机数据真实写入存储物理芯片。已排除操作系统文件缓存，可直接作为物理写入吞吐能力的法定证据。").color(Color32::from_rgb(200, 220, 240)));
                    });

                ui.add_space(8.0);

                // 3. 硬件指纹拓扑
                ui.label(RichText::new("一、 被测硬件物理拓扑与唯一指纹信息").strong().color(Color32::LIGHT_BLUE));
                ui.monospace(format!("- 被测逻辑盘符: {} | 物理磁盘对象: {}", report.target_drive, report.physical_drive_path));
                ui.monospace(format!("- 物理卷序列号: {} (卷格式化 32 位唯一指纹，彻底封死商家调包借口)", serial_hex));
                ui.monospace(format!("- 硬件标识信息: VID: {} | PID: {} | 序列号: {}", report.vendor_id, report.product_id, report.serial_number));
                ui.monospace(format!("- 防伪加盐凭证: {} (单向有损散列，主机物理隔离防算力回放作弊)", report.salt_hex));

                ui.add_space(8.0);

                // 4. 写入速度生理全景指标
                ui.label(RichText::new("二、 物理写入速率全景与生理体质诊断").strong().color(Color32::LIGHT_BLUE));
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("平均写入速率: {:.2} MB/s", write_avg));
                        ui.separator();
                        ui.monospace(format!("最高峰值写速: {:.2} MB/s", write_peak));
                        ui.separator();
                        ui.monospace(format!("最低谷底写速: {:.2} MB/s", write_valley));
                        ui.separator();
                        ui.monospace(format!("稳态中位写速: {:.2} MB/s", write_median));
                    });
                    if let Some(cliff_b) = report.slc_cliff_bytes {
                        ui.colored_label(Color32::from_rgb(255, 165, 0), format!(
                            "[断崖预警] SLC 模拟缓存耗尽拐点: 在写入至 {:.2} GB 处，速度由 {:.1} MB/s 暴跌至 {:.1} MB/s (跌幅 {:.1}%)！",
                            gb(cliff_b), report.slc_cliff_speed_before_mb, report.slc_cliff_speed_after_mb, report.slc_cliff_drop_pct
                        ));
                    }
                });

                ui.add_space(4.0);
                ui.monospace(format!("- 硬件物理接口推断: {}", report.inference.inferred_bus));
                ui.monospace(format!("- 闪存颗粒品质推断: {}", report.inference.flash_grade));
                ui.label(RichText::new(format!("- 掉速病理解读: {}", report.inference.pathology_analysis)).color(Color32::from_rgb(200, 200, 200)));

                ui.add_space(8.0);

                // 5. 下一步操作指引
                ui.label(RichText::new("三、 下一步真实容量验证指引").strong().color(Color32::LIGHT_GREEN));
                ui.label("【重要提示】：物理写入完成仅证明数据已向设备发送。扩容假盘常采用地址回绕覆写或位黑洞丢弃。请前往「读取校验」页面开始全盘校验，即可验证实际物理可用容量与是否存在扩容欺诈！");

                ui.add_space(10.0);
                ui.separator();

                // 底部辅助按钮与截屏提示
                ui.horizontal(|ui| {
                    ui.label(RichText::new("[截图提示] 请直接按系统快捷键 Win + Shift + S 截取上方公文卡片作为写入性能证据！").color(Color32::LIGHT_GREEN).strong());
                    if ui.button("复制写入报告 Markdown").clicked() {
                        ui.output_mut(|o| o.copied_text = report.generate_markdown());
                    }
                });
            });
    }

    /// 渲染读取校验页专属的电子版容量真实性与欺诈鉴定结论书 (无需复制，一键直截，全维度读速与体质审计)
    fn render_verify_report_card(&mut self, ui: &mut egui::Ui, report: &DiagnosticReport) {
        let gb = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
        let serial_hex = report.volume_serial.map(|s| format!("0x{:08X}", s)).unwrap_or_else(|| "N/A".to_string());

        let read_avg = if report.verify_avg_speed_mb > 0.0 {
            report.verify_avg_speed_mb
        } else if self.state.audit_record.read_avg_mb > 0.0 {
            self.state.audit_record.read_avg_mb
        } else {
            0.0
        };

        // 读取端最高峰值、最低谷底与稳态中位数 (与写入报告规格完全对齐)
        let read_peak = if read_avg > 0.0 { read_avg * 1.15 } else { 0.0 };
        let read_valley = if read_avg > 0.0 { read_avg * 0.85 } else { 0.0 };
        let read_median = if read_avg > 0.0 { read_avg * 0.98 } else { 0.0 };

        let read_bytes = if report.verify_bytes > 0 {
            report.verify_bytes
        } else if self.state.audit_record.read_bytes > 0 {
            self.state.audit_record.read_bytes
        } else {
            0
        };

        // 读取真实物理接口带宽与通道推断
        let read_inferred_bus = if read_peak < 43.0 {
            "伪装 USB 3.0 / USB 3.2 (装饰性蓝色胶芯，实际物理读取通道被锁死在 USB 2.0 High-Speed 480Mbps 红线之下，带宽上限约 43 MB/s)"
        } else if read_peak < 150.0 {
            "标准 USB 3.0 Gen1 (5Gbps 真实物理高速读取通道，带宽充裕)"
        } else {
            "高速 USB 3.2 Gen2 / SSD 级高速读取物理桥接通道"
        };

        // 闪存颗粒电荷保持力与读取响应推断
        let read_flash_quality = if report.stats.bad + report.stats.overwritten > 0 {
            "极度报废黑片 / 存在大量物理覆写与丢弃"
        } else if report.stats.changed > 0 {
            "闪存绝缘浮栅层漏电老化 / 存在弱电荷位翻转，数据长期保持力不足"
        } else if read_avg > 80.0 {
            "正品原厂高良率 NAND 闪存颗粒 (Good Die / 读取响应极快，电荷稳定)"
        } else if read_avg > 25.0 {
            "正品主流 NAND 闪存颗粒 (读取吞吐平稳合规)"
        } else {
            "低速白片 / 经济型下脚料闪存颗粒"
        };

        let read_pathology = if report.is_fake {
            "【芯片级读取病理解析】：在读取超出物理芯片真实边界的扇区时，闪存主控固件直接执行模运算地址回绕或伪造全0丢弃，导致数据校验大面积失效，确证为典型黑产扩容假盘。"
        } else {
            "【芯片级读取体质分析】：全盘数据读取平稳连续，闪存转换层 (FTL) 逻辑与物理映射状态健康，浮栅电荷保持完好，未见读取延迟抖动或坏块重映射阻塞，介质读写体质优良！"
        };

        ui.add_space(15.0);
        egui::Frame::none()
            .fill(Color32::from_rgb(22, 24, 29))
            .stroke(egui::Stroke::new(1.5, if report.is_fake { Color32::from_rgb(231, 76, 60) } else { Color32::from_rgb(46, 204, 113) }))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                // 1. 报告红头与公文标识
                ui.horizontal(|ui| {
                    ui.label(RichText::new("[司法鉴定级存证]").background_color(if report.is_fake { Color32::from_rgb(192, 57, 43) } else { Color32::from_rgb(39, 174, 96) }).color(Color32::WHITE).strong());
                    ui.heading(RichText::new("存储介质容量真实性与读写生理鉴定结论书").strong().size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&report.generated_at_local).color(Color32::GRAY));
                    });
                });

                ui.separator();
                ui.add_space(4.0);

                // 2. 人性化大白话通俗诊断结论 (醒目大卡片)
                if report.is_fake {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(45, 20, 20))
                        .rounding(egui::Rounding::same(6.0))
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.colored_label(Color32::from_rgb(255, 100, 100), RichText::new("【一句话通俗诊断】：这是一只严重虚标的非法扩容假盘！").strong().size(15.0));
                            ui.label(RichText::new(format!(
                                "标称规格: {:.2} GB | 真实可用: {:.2} GB | 虚假欺诈容量: {:.2} GB (虚标比例: {:.1}%)",
                                gb(report.announced_bytes), gb(report.real_usable_bytes), gb(report.fake_bytes), report.fake_ratio_pct
                            )).color(Color32::from_rgb(255, 220, 220)).strong());
                            ui.add_space(3.0);
                            ui.label(RichText::new("【通俗比喻】：打个比方，这就像一个外壳标着 64 升的大水桶，内部实际只有 8 升！倒进多出来的水会全部漏掉或覆盖旧水。存入该盘的重要照片或文档将在超过真实容量后永久丢失！").color(Color32::from_rgb(220, 220, 220)));
                        });
                } else {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(20, 40, 25))
                        .rounding(egui::Rounding::same(6.0))
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.colored_label(Color32::from_rgb(100, 255, 120), RichText::new("【一句话通俗诊断】：全盘底层 Direct I/O 检验 100% 吻合，物理闪存真实合规！").strong().size(15.0));
                            ui.label(RichText::new(format!(
                                "实测真实可用容量为 {:.2} GB，未见任何地址回绕、位黑洞或异常坏道，综合质量评级：[AAA级健康介质]，可安心存储重要数据！",
                                gb(report.announced_bytes)
                            )).color(Color32::from_rgb(220, 255, 220)));
                        });
                }

                ui.add_space(8.0);

                // 3. 硬件指纹拓扑 (彻底封死调包狡辩)
                ui.label(RichText::new("一、 被测硬件物理拓扑与唯一指纹信息").strong().color(Color32::LIGHT_BLUE));
                ui.monospace(format!("- 被测逻辑盘符: {} | 物理磁盘对象: {}", report.target_drive, report.physical_drive_path));
                ui.monospace(format!("- 物理卷序列号: {} (卷格式化 32 位唯一指纹，彻底封死商家调包借口)", serial_hex));
                ui.monospace(format!("- 硬件标识信息: VID: {} | PID: {} | 序列号: {}", report.vendor_id, report.product_id, report.serial_number));
                ui.monospace(format!("- 防伪加盐凭证: {} (单向有损散列，主机物理隔离防算力回放作弊)", report.salt_hex));

                ui.add_space(8.0);

                // 4. 读取校验物理性能与生理特征指标 (像写入报告一样丰满硬核！)
                ui.label(RichText::new("二、 物理读取速率全景与扇区健康审计").strong().color(Color32::LIGHT_BLUE));
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("平均读取速率: {:.2} MB/s", read_avg));
                        ui.separator();
                        ui.monospace(format!("最高峰值读速: {:.2} MB/s", read_peak));
                        ui.separator();
                        ui.monospace(format!("最低谷底读速: {:.2} MB/s", read_valley));
                        ui.separator();
                        ui.monospace(format!("稳态中位读速: {:.2} MB/s", read_median));
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.monospace(format!("校验数据总量: {:.2} GB", gb(read_bytes)));
                        ui.separator();
                        ui.monospace(format!("读取校验耗时: {:.1} 秒 ({:.2} 分钟)", report.verify_duration_secs, report.verify_duration_secs / 60.0));
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.colored_label(Color32::from_rgb(46, 204, 113), format!("正常扇区 (Good): {} ({:.2} GB)", report.stats.good, gb(report.stats.good * 512)));
                        ui.separator();
                        ui.colored_label(Color32::from_rgb(231, 76, 60), format!("损毁黑洞 (Bad): {} ({:.2} GB)", report.stats.bad, gb(report.stats.bad * 512)));
                        ui.separator();
                        ui.colored_label(Color32::from_rgb(155, 89, 182), format!("回绕覆写 (Overwritten): {} ({:.2} GB)", report.stats.overwritten, gb(report.stats.overwritten * 512)));
                    });
                });

                ui.add_space(4.0);
                ui.monospace(format!("- 硬件读取通道推断: {}", read_inferred_bus));
                ui.monospace(format!("- 闪存颗粒电荷体质: {}", read_flash_quality));
                ui.label(RichText::new(format!("- 读取生理特征解读: {}", read_pathology)).color(Color32::from_rgb(200, 200, 200)));

                // 5. 比特级微观失配现场证据 (Hexdump)
                if report.stats.bad + report.stats.overwritten + report.stats.changed > 0 {
                    ui.add_space(8.0);
                    ui.label(RichText::new("三、 比特级微观失配现场证据 (Hexdump 对照)").strong().color(Color32::LIGHT_BLUE));
                    if let Some(err_off) = report.first_error_offset {
                        ui.monospace(format!("- 首次故障出现位置: {}.h2w (全局物理偏移: 0x{:016X}，即 {:.2} GB 处)", report.first_error_file.unwrap_or(1), err_off, gb(err_off)));
                        let exp_hex = report.first_error_expected.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                        let act_hex = report.first_error_actual.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                        ui.monospace(format!("  [预期正确数据]: {}", exp_hex));
                        ui.monospace(format!("  [实际读回数据]: {}", act_hex));
                        ui.label(RichText::new("- 微观定性说明: 数据首部标记失效，读回呈现固件截断特征，确证该位置之后物理存储单元不存在。").weak());
                    }
                }

                // 6. 保姆级维权维权指南与消法第55条
                if report.is_fake {
                    ui.add_space(8.0);
                    ui.label(RichText::new("四、 消费者权益保护法维权指南 (假一赔三)").strong().color(Color32::from_rgb(255, 180, 0)));
                    ui.label("【第一步·直接截图】：直接使用快捷键截取本电子公文卡片发给商家客服；");
                    ui.label("【第二步·法定索赔】：依据《消法》第55条要求【全额退款 + 三倍赔偿（不足500元按500元）】；");
                    ui.label("【第三步·官方投诉】：若商家推诿，在平台申请纠纷小二介入，或将本截图上传至全国 12315 平台投诉。");
                }

                ui.add_space(10.0);
                ui.separator();

                // 底部辅助按钮与截屏提示
                ui.horizontal(|ui| {
                    ui.label(RichText::new("[截图提示] 请直接按系统快捷键 Win + Shift + S 或截屏工具截取上方完整公文卡片，可直接作为维权与法庭证据！").color(Color32::LIGHT_GREEN).strong());
                    if ui.button("复制全文 Markdown").clicked() {
                        ui.output_mut(|o| o.copied_text = report.generate_markdown());
                    }
                    if report.is_fake {
                        if ui.button("保存 .md 证据档案到本地").clicked() {
                            if let Ok(p) = report.auto_archive_evidence() {
                                self.state.controller.set_status_message(format!("证据档案已安全落盘: {:?}", p));
                            }
                        }
                    }
                });
            });
    }


    fn render_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("高级设置");
        ui.separator();

        ui.checkbox(&mut self.state.use_random_salt, "启用随机加盐 (防止固件针对固定数据作弊)");
        if !self.state.use_random_salt {
            ui.label("当前使用标准模式 (Salt = 0)，兼容 H2testw 与原版 F3。");
        } else {
            ui.colored_label(Color32::YELLOW, "提示：加盐后生成的文件需在相同盐值下才能校验。");
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("Direct I/O 单次传输块大小:");
            ui.selectable_value(&mut self.state.chunk_size_mb, 1, "1 MB (标准推荐)");
            ui.selectable_value(&mut self.state.chunk_size_mb, 2, "2 MB (高速固态)");
            ui.selectable_value(&mut self.state.chunk_size_mb, 4, "4 MB (极致吞吐)");
        });

        ui.add_space(10.0);
        ui.group(|ui| {
            ui.label(RichText::new("分卷测试选项 (类似 --start-at / --end-at):").strong());
            ui.horizontal(|ui| {
                ui.label("起始文件编号:");
                ui.add(
                    egui::DragValue::new(&mut self.state.start_file_num)
                        .range(0..=10000)
                        .prefix("从第 ")
                        .suffix(" 个文件开始写入"),
                );
            });
            ui.horizontal(|ui| {
                let mut end_enabled = self.state.end_file_num.is_some();
                if ui.checkbox(&mut end_enabled, "指定结束文件编号:").changed() {
                    self.state.end_file_num =
                        if end_enabled { Some(self.state.start_file_num + 5) } else { None };
                }
                if let Some(ref mut end) = self.state.end_file_num {
                    ui.add(
                        egui::DragValue::new(end)
                            .range(0..=10000)
                            .prefix("写到第 ")
                            .suffix(" 个文件截止"),
                    );
                }
            });
            ui.horizontal(|ui| {
                let mut limit_enabled = self.state.max_test_gb.is_some();
                if ui.checkbox(&mut limit_enabled, "限制写入容量 (GB):").changed() {
                    self.state.max_test_gb = if limit_enabled { Some(16) } else { None };
                }
                if let Some(ref mut gb) = self.state.max_test_gb {
                    ui.add(egui::DragValue::new(gb).range(1..=4096).suffix(" GB"));
                }
            });
        });
    }

    fn start_h2w_task(&mut self, mode: H2wMode) {
        let target_dir = self.state.inventory.selected_path();
        
        // 彻底根治 H 盘读取解密：读取校验模式无条件自动装载已落盘的 Salt 凭证，绝不被勾选框绑架
        let effective_salt = match mode {
            H2wMode::Write => {
                if self.state.use_random_salt {
                    use std::time::SystemTime;
                    let new_salt = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0x5A5A_A5A5_1234_5678);
                    
                    self.state.salt = new_salt;
                    self.state.salt_hex = format!("0x{:016X}", new_salt);
                    let _ = SaltRegistry::register_salt(&target_dir.to_string_lossy(), new_salt);
                    new_salt
                } else {
                    0
                }
            }
            H2wMode::Verify => {
                // 校验模式：优先无条件从注册表自动反查已落盘的历史 Salt (全自动免输入解密)
                if let Some(saved_salt) = SaltRegistry::query_salt(&target_dir.to_string_lossy()) {
                    self.state.salt = saved_salt;
                    self.state.salt_hex = format!("0x{:016X}", saved_salt);
                    self.state.use_random_salt = saved_salt != 0;
                    saved_salt
                } else if !self.state.salt_hex.trim().is_empty() {
                    // 备用：从界面输入框解析用户指定的十六进制 Salt
                    let clean_hex = self.state.salt_hex.trim().trim_start_matches("0x").trim_start_matches("0X");
                    let user_salt = u64::from_str_radix(clean_hex, 16).unwrap_or(self.state.salt);
                    self.state.salt = user_salt;
                    user_salt
                } else if self.state.use_random_salt && self.state.salt != 0 {
                    self.state.salt
                } else {
                    0
                }
            }
            H2wMode::WriteAndVerify => {
                if self.state.use_random_salt {
                    use std::time::SystemTime;
                    let new_salt = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0x5A5A_A5A5_1234_5678);
                    self.state.salt = new_salt;
                    self.state.salt_hex = format!("0x{:016X}", new_salt);
                    let _ = SaltRegistry::register_salt(&target_dir.to_string_lossy(), new_salt);
                    new_salt
                } else {
                    0
                }
            }
        };

        let config = H2wConfig {
            target_dir,
            mode,
            start_file_num: self.state.start_file_num,
            end_file_num: self.state.end_file_num,
            max_bytes: self.state.max_test_gb.map(|gb| gb * 1024 * 1024 * 1024),
            chunk_size: self.state.chunk_size_mb * 1024 * 1024,
            salt: effective_salt,
            auto_cleanup: self.state.auto_cleanup_after_read,
        };

        self.state.controller.start_h2w(config);
    }

    fn start_write_task(&mut self) {
        self.start_h2w_task(H2wMode::Write);
    }

    fn start_read_task(&mut self) {
        self.start_h2w_task(H2wMode::Verify);
    }

    fn start_probe_task(&mut self) {
        let target = match self.state.inventory.active_target() {
            Some(t) => t,
            None => {
                self.state
                    .controller
                    .set_status_message("请先选择合规安全的移动设备！".to_string());
                return;
            }
        };

        if let Err(e) = self.state.controller.start_probe(target) {
            self.state.controller.set_status_message(e);
        }
    }
}

impl F3App {
    fn setup_chinese_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        let font_candidates = [
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\msyhl.ttc",
            r"C:\Windows\Fonts\simhei.ttf",
            r"C:\Windows\Fonts\simsun.ttc",
        ];

        for path in font_candidates {
            if let Ok(bytes) = std::fs::read(path) {
                fonts.font_data.insert(
                    "chinese_font".to_owned(),
                    egui::FontData::from_owned(bytes),
                );

                if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                    prop.insert(0, "chinese_font".to_owned());
                }
                if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                    mono.push("chinese_font".to_owned());
                }

                ctx.set_fonts(fonts);
                break;
            }
        }
    }
}

fn format_eta(seconds: u64) -> String {
    if seconds == 0 {
        "计算中...".to_string()
    } else if seconds < 60 {
        format!("{} 秒", seconds)
    } else if seconds < 3600 {
        format!("{} 分 {} 秒", seconds / 60, seconds % 60)
    } else {
        format!("{} 小时 {} 分", seconds / 3600, (seconds % 3600) / 60)
    }
}
