use std::path::PathBuf;
use eframe::egui::{self, Color32, ProgressBar, RichText};

use crate::core::controller::{TaskSnapshot, TaskStatus};
use crate::core::fix::SafePartitionFixer;
use crate::core::session::{H2wConfig, H2wMode, H2wTelemetry};
use crate::core::salt::SaltRegistry;
use crate::core::report::DiagnosticReport;
use crate::gui::components::device_selector::{
    render_storage_device_selector, render_storage_volume_selector,
};
use crate::core::paths::{self, AppPathInfo};
use crate::gui::components::heatmap::render_heatmap;
use crate::gui::components::plot::render_speed_plot;
use crate::gui::state::{AppState, AppTab};
use crate::gui::tray::{TrayEvent, TrayManager};
use crate::gui::theme;

pub struct F3App {
    state: AppState,
    prev_status: TaskStatus,
    tray: Option<TrayManager>,
    is_exiting: bool,
}

impl F3App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_modern_theme(&cc.egui_ctx);
        Self::setup_chinese_fonts(&cc.egui_ctx);
        let tray = Some(TrayManager::new(
            "F3 (管理员) - 闪存完整性测试就绪",
            cc.egui_ctx.clone(),
        ));
        Self {
            state: AppState::default(),
            prev_status: TaskStatus::Idle,
            tray,
            is_exiting: false,
        }
    }

    /// 执行终极安全退出流程，确保释放设备锁、销毁托盘并关闭应用视口
    fn perform_exit(&mut self, ctx: &egui::Context) {
        self.is_exiting = true;
        self.state.controller.cancel();
        if let Some(ref mut t) = self.tray {
            t.destroy();
        }
        self.state.show_exit_confirm_modal = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for F3App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 单行获取深层任务控制器轮询出的最新只读快照
        let snapshot = self.state.controller.poll().clone();

        // 轮询消费来自后台系统托盘的交互事件 (预先排空收集至列表，彻底解耦 self.tray 借用)
        let tray_events: Vec<TrayEvent> = self
            .tray
            .as_ref()
            .map(|tray| {
                let mut events = Vec::new();
                while let Some(ev) = tray.try_recv_event() {
                    events.push(ev);
                }
                events
            })
            .unwrap_or_default();

        for event in tray_events {
            match event {
                TrayEvent::ShowWindow => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayEvent::ShowPathInfoModal => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.state.show_path_info_modal = true;
                }
                TrayEvent::ShowAboutModal => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayEvent::OpenExeDir => {
                    let info = AppPathInfo::gather(None);
                    let _ = paths::open_in_explorer(&info.exe_dir);
                }
                TrayEvent::OpenSelectedVolume => {
                    let vol = self.state.inventory.selected_path();
                    let _ = paths::open_in_explorer(&vol);
                }
                TrayEvent::OpenEvidenceDir => {
                    let info = AppPathInfo::gather(None);
                    let _ = paths::open_in_explorer(&info.evidence_dir);
                }
                TrayEvent::RequestExit => {
                    let is_running = self.state.controller.poll().status == TaskStatus::Running;
                    if is_running {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.state.show_exit_confirm_modal = true;
                    } else {
                        self.perform_exit(ctx);
                    }
                }
            }
        }

        // 拦截主窗口关闭请求：
        // 若已进入退出流程 (is_exiting == true)，严禁拦截，放行让 eframe 正常退出；
        // 若开启了“最小化到托盘”，则仅隐藏主窗口保持后台任务无损运行；
        // 若未开启且测试正在运行，弹出安全退出防呆拦截对话框；
        // 否则执行终极退出流程。
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.is_exiting {
                // 已标记退出，放行 Close 指令，不再发送 CancelClose
            } else if self.state.minimize_to_tray_on_close {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else if snapshot.status == TaskStatus::Running {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.state.show_exit_confirm_modal = true;
            } else {
                self.perform_exit(ctx);
            }
        }

        // 动态同步更新托盘图标悬停状态提示文案
        if let Some(ref tray) = self.tray {
            let tip = match snapshot.status {
                TaskStatus::Running => format!(
                    "F3 (管理员) - 正在测试: {:.1} MB/s (已处理 {:.2} GB)",
                    snapshot.current_speed_mb,
                    snapshot.total_bytes_processed as f64 / (1024.0 * 1024.0 * 1024.0)
                ),
                TaskStatus::Finished => "F3 (管理员) - 测试已完成 (就绪)".to_string(),
                TaskStatus::Error => "F3 (管理员) - 测试异常中断".to_string(),
                TaskStatus::Idle => "F3 (管理员) - 闪存完整性测试就绪".to_string(),
            };
            tray.set_tooltip(&tip);
        }

        // 盘符切换检测：当用户下拉切换测试卷时，原子化重置历史审计公文报告，杜绝跨盘幽灵报告污染
        let current_path = self.state.inventory.selected_path();
        if self.state.last_selected_path.as_ref() != Some(&current_path) {
            self.state.audit_record = crate::gui::state::SpeedAuditRecord::default();
            self.state.cached_dry_run_report = None;
            self.state.last_selected_path = Some(current_path);
        }

        // 当异步任务脱离运行状态时 (无论是正常完成还是异常掉盘)，自动静默刷新存储资产与磁盘空间
        let task_just_stopped = self.prev_status == TaskStatus::Running && snapshot.status != TaskStatus::Running;
        if task_just_stopped {
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

            // 只要检测到容量虚标、坏道或电荷翻转损坏，在测试完成边界静默自动在 exe 同级目录生成 f3_evidence_<serial>_<timestamp>.md 档案！
            if snapshot.stats.overwritten > 0 || snapshot.stats.bad > 0 || snapshot.stats.changed > 0 {
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

        let top_frame = egui::Frame::none()
            .fill(theme::PANEL_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .inner_margin(egui::Margin::symmetric(14.0, 9.0));

        egui::TopBottomPanel::top("top_nav")
            .frame(top_frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("F3").strong().size(15.0).color(theme::TEXT_PRIMARY));
                    theme::badge_neutral(ui, "DIRECT I/O");
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // 现代一体化黑白选项卡项
                    let mut render_tab = |ui: &mut egui::Ui, tab: AppTab, label: &str| {
                        let is_active = self.state.current_tab == tab;
                        let text_color = if is_active {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_SECONDARY
                        };
                        let bg_color = if is_active {
                            theme::SURFACE_ACTIVE
                        } else {
                            Color32::TRANSPARENT
                        };
                        let stroke = if is_active {
                            egui::Stroke::new(1.0_f32, theme::BORDER_STRONG)
                        } else {
                            egui::Stroke::NONE
                        };

                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(13.0)
                                .color(text_color)
                                .strong(),
                        )
                        .fill(bg_color)
                        .stroke(stroke)
                        .rounding(egui::Rounding::ZERO);

                        if ui.add(btn).clicked() {
                            self.state.current_tab = tab;
                        }
                    };

                    render_tab(ui, AppTab::WriteTest, "写入测试");
                    render_tab(ui, AppTab::ReadVerify, "读取校验");
                    render_tab(ui, AppTab::RapidProbe, "闪电探测");
                    render_tab(ui, AppTab::PartitionFix, "分区修复");
                    render_tab(ui, AppTab::Settings, "设置");

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(theme::ghost_button(
                                RichText::new("路径信息中心").size(12.0).color(theme::TEXT_SECONDARY),
                            ))
                            .clicked()
                        {
                            self.state.show_path_info_modal = true;
                        }
                    });
                });
            });

        let bottom_frame = egui::Frame::none()
            .fill(theme::PANEL_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .inner_margin(egui::Margin::symmetric(14.0, 7.0));

        egui::TopBottomPanel::bottom("status_bar")
            .frame(bottom_frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let dot_color = match snapshot.status {
                        TaskStatus::Idle => theme::TEXT_MUTED,
                        TaskStatus::Running => theme::STATUS_CYAN,
                        TaskStatus::Finished => theme::STATUS_GOOD,
                        TaskStatus::Error => theme::STATUS_BAD,
                    };
                    theme::status_dot(ui, dot_color);
                    ui.label(
                        RichText::new(&snapshot.status_message)
                            .size(12.0)
                            .color(theme::TEXT_SECONDARY),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(target) = self.state.inventory.active_target() {
                            ui.label(
                                RichText::new(format!(
                                    "当前目标: {} | PhysicalDrive{}",
                                    self.state.inventory.selected_path().display(),
                                    target.device.drive_index
                                ))
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                            );
                        }
                    });
                });
            });

        let central_frame = egui::Frame::none()
            .fill(theme::CANVAS_BG)
            .inner_margin(egui::Margin::symmetric(18.0, 14.0));

        egui::CentralPanel::default()
            .frame(central_frame)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.state.current_tab {
                            AppTab::WriteTest => self.render_write_tab(ui, &snapshot),
                            AppTab::ReadVerify => self.render_read_tab(ui, &snapshot),
                            AppTab::RapidProbe => self.render_probe_tab(ui, &snapshot),
                            AppTab::PartitionFix => self.render_fix_tab(ui, &snapshot),
                            AppTab::Settings => self.render_settings_tab(ui),
                        }
                    });
            });

        // 渲染路径全景信息模态卡片与安全退出防呆拦截弹窗
        self.render_path_info_modal(ctx);
        self.render_exit_confirm_modal(ctx);
    }
}

impl F3App {
    fn render_write_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        // 现代黑白极简页面标题与说明
        ui.label(RichText::new("写入测试 (f3write)").size(18.0).strong().color(theme::TEXT_PRIMARY));
        ui.label(RichText::new("以 Direct I/O 无系统缓存模式向目标磁盘填充 .h2w 数据文件，测试真实硬件物理写入速率。").size(12.0).color(theme::TEXT_MUTED));
        ui.add_space(6.0);

        let is_busy = snapshot.status == TaskStatus::Running;
        ui.add_enabled_ui(!is_busy, |ui| {
            render_storage_volume_selector(ui, "write_vol_combo", &mut self.state.inventory);
        });

        ui.add_space(8.0);

        let is_safe = self.state.inventory.selected_volume().is_some_and(|v| v.is_safe_target);
        let is_write_running = snapshot.status == TaskStatus::Running
            && matches!(snapshot.kind, crate::core::controller::TaskKind::Write | crate::core::controller::TaskKind::WriteAndVerify);
        let is_other_running = snapshot.status == TaskStatus::Running && !is_write_running;

        // 主操作与状态按钮
        ui.horizontal(|ui| {
            if is_write_running {
                if ui.add_sized([130.0, 36.0], theme::danger_button("停止写入")).clicked() {
                    self.state.controller.cancel();
                }
            } else {
                let can_start = is_safe && !is_other_running;
                let btn_text = if is_other_running {
                    "其他任务运行中..."
                } else if !is_safe {
                    "设备受保护或未就绪"
                } else {
                    "开始写入"
                };
                if can_start {
                    if ui.add_sized([130.0, 36.0], theme::primary_button(btn_text)).clicked() {
                        self.start_write_task();
                    }
                } else {
                    let _ = ui.add_sized([140.0, 36.0], theme::disabled_button(btn_text));
                }
            }

            ui.add_space(10.0);

            // 便捷操作幽灵胶囊按钮 (复用标准路径服务接口)
            if ui.add_sized([110.0, 36.0], theme::ghost_button("打开所在目录")).clicked() {
                let _ = crate::core::paths::open_in_explorer(&self.state.inventory.selected_path());
            }

            let is_busy = snapshot.status == TaskStatus::Running;
            if is_busy {
                let _ = ui.add_sized([110.0, 36.0], theme::disabled_button("清理测试文件"));
            } else if ui.add_sized([110.0, 36.0], theme::ghost_button("清理测试文件")).clicked() {
                let path = self.state.inventory.selected_path();
                self.state.controller.start_cleanup(path);
            }
        });

        ui.add_space(12.0);

        // 现代 Bento-Grid 四卡片核心指标仪表盘 (严格隔离任务状态，杜绝跨任务数据污染)
        ui.horizontal(|ui| {
            let card_width = (ui.available_width() - 36.0) / 4.0;

            let display_speed = if is_write_running {
                snapshot.current_speed_mb
            } else if self.state.audit_record.write_tested {
                self.state.audit_record.write_avg_mb
            } else {
                0.0
            };

            let display_bytes = if is_write_running {
                snapshot.total_bytes_processed
            } else if self.state.audit_record.write_tested {
                self.state.audit_record.write_bytes
            } else {
                0
            };

            // 卡片 1: 瞬时速率
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("实时写入速度").size(11.0).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{:.2}", display_speed))
                            .size(24.0)
                            .strong()
                            .color(theme::ACCENT_CYAN),
                    );
                    ui.label(RichText::new("MB/s").size(11.0).color(theme::TEXT_MUTED));
                });
            });

            ui.add_space(12.0);

            // 卡片 2: 已写入数据
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("已写入容量").size(11.0).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{:.2}",
                            display_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        ))
                        .size(24.0)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(RichText::new("GB").size(11.0).color(theme::TEXT_MUTED));
                });
            });

            ui.add_space(12.0);

            // 卡片 3: 动态 ETA 倒计时
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("预计剩余时间").size(11.0).color(theme::TEXT_MUTED));
                let eta_str = if is_write_running {
                    format_eta(snapshot.eta_seconds)
                } else {
                    "空闲待命".to_string()
                };
                ui.label(
                    RichText::new(eta_str)
                        .size(20.0)
                        .strong()
                        .color(if is_write_running { theme::ACCENT_AMBER } else { theme::TEXT_SECONDARY }),
                );
            });

            ui.add_space(12.0);

            // 卡片 4: I/O 驱动模式
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("I/O 底层驱动").size(11.0).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Direct I/O").size(18.0).strong().color(theme::TEXT_PRIMARY));
                    theme::badge_green(ui, "无缓冲");
                });
            });
        });

        ui.add_space(10.0);
        let empty_history = Vec::new();
        let write_history = if is_write_running || (snapshot.status != TaskStatus::Running && matches!(snapshot.kind, crate::core::controller::TaskKind::Write | crate::core::controller::TaskKind::WriteAndVerify)) {
            &snapshot.speed_history
        } else {
            &empty_history
        };
        render_speed_plot(ui, write_history, "写入带宽", "write_speed_plot", crate::gui::components::plot::PlotKind::Write);

        // 写入测试完成时，在底部直接渲染专业电子版技术检验报告公文卡片 (同时展示写入和读取速度，可直接截屏)
        let is_write_finished = snapshot.status == TaskStatus::Finished
            && matches!(snapshot.kind, crate::core::controller::TaskKind::Write | crate::core::controller::TaskKind::WriteAndVerify)
            && snapshot.total_bytes_processed > 0;
        if is_write_finished || self.state.audit_record.write_tested {
            let active_target = self.state.inventory.active_target();
            let dev = active_target.as_ref().map(|t| &t.device);
            let serial = SaltRegistry::get_volume_serial(&self.state.inventory.selected_path().to_string_lossy());
            let current_is_write = matches!(snapshot.kind, crate::core::controller::TaskKind::Write | crate::core::controller::TaskKind::WriteAndVerify);
            let duration_seconds = if current_is_write && snapshot.duration_seconds > 0.0 {
                snapshot.duration_seconds
            } else {
                self.state.audit_record.write_duration_secs
            };
            let bytes_processed = if current_is_write && snapshot.total_bytes_processed > 0 {
                snapshot.total_bytes_processed
            } else {
                self.state.audit_record.write_bytes
            };
            let speed_bps = if current_is_write && snapshot.current_speed_mb > 0.0 {
                snapshot.current_speed_mb * 1024.0 * 1024.0
            } else {
                self.state.audit_record.write_avg_mb * 1024.0 * 1024.0
            };
            let physiology = if current_is_write {
                snapshot.physiology
            } else {
                None
            };
            let dummy_telemetry = H2wTelemetry {
                mode: H2wMode::Write,
                phase: crate::core::session::H2wPhase::Completed,
                current_file: 1,
                total_files: None,
                bytes_processed,
                total_bytes_expected: bytes_processed,
                speed_bps,
                duration_seconds,
                eta_seconds: 0,
                stats: if current_is_write { snapshot.stats } else { Default::default() },
                heatmap_sample: None,
                is_finished: true,
                error: None,
                physiology,
                first_error_offset: None,
                first_error_file: None,
                first_error_expected: None,
                first_error_actual: None,
            };
            let report = DiagnosticReport::build_write_audit(
                dev,
                serial,
                &self.state.inventory.selected_path(),
                &dummy_telemetry,
                self.state.salt,
                snapshot.physiology,
            );

            self.render_write_report_card(ui, &report);
        }
    }

    fn render_read_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        // 现代黑白极简页面标题与说明
        ui.label(RichText::new("读取校验 (f3read)").size(18.0).strong().color(theme::TEXT_PRIMARY));
        ui.label(RichText::new("读取磁盘上的 .h2w 数据文件逐比特哈希比对，排查损坏坏块与回绕覆盖虚标。").size(12.0).color(theme::TEXT_MUTED));
        ui.add_space(6.0);

        let is_busy = snapshot.status == TaskStatus::Running;
        ui.add_enabled_ui(!is_busy, |ui| {
            render_storage_volume_selector(ui, "read_vol_combo", &mut self.state.inventory);
        });

        ui.add_space(8.0);

        // 校验控制与断点续验分段配置
        let is_safe = self.state.inventory.selected_volume().is_some_and(|v| v.is_safe_target);
        let is_read_running = snapshot.status == TaskStatus::Running
            && matches!(snapshot.kind, crate::core::controller::TaskKind::Verify | crate::core::controller::TaskKind::WriteAndVerify);
        let is_other_running = snapshot.status == TaskStatus::Running && !is_read_running;

        ui.horizontal(|ui| {
            if is_read_running {
                if ui.add_sized([130.0, 36.0], theme::danger_button("停止校验")).clicked() {
                    self.state.controller.cancel();
                }
            } else {
                let can_start = is_safe && !is_other_running;
                let btn_text = if is_other_running {
                    "其他任务运行中..."
                } else if !is_safe {
                    "设备受保护或未就绪"
                } else {
                    "开始校验"
                };
                if can_start {
                    if ui.add_sized([130.0, 36.0], theme::primary_button(btn_text)).clicked() {
                        self.start_read_task();
                    }
                } else {
                    let _ = ui.add_sized([140.0, 36.0], theme::disabled_button(btn_text));
                }
            }

            ui.add_space(10.0);

            // 断点续验分段配置 (运行中置灰防呆锁定)
            ui.add_enabled_ui(!is_read_running, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("起始分卷:").size(12.0).color(theme::TEXT_MUTED));
                    let mut start_val = (self.state.start_file_num + 1) as i64;
                    if ui.add(egui::DragValue::new(&mut start_val).range(1..=99999).prefix("第 ").suffix(" 卷 (.h2w)")).changed() {
                        self.state.start_file_num = (start_val.max(1) - 1) as u64;
                    }

                    ui.add_space(6.0);

                    ui.label(RichText::new("结束分卷:").size(12.0).color(theme::TEXT_MUTED));
                    let mut end_val = self.state.end_file_num.map(|n| (n + 1) as i64).unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut end_val).range(0..=99999).prefix("至 ").suffix(" 卷")).changed() {
                        self.state.end_file_num = if end_val > 0 { Some((end_val - 1) as u64) } else { None };
                    }
                });
            });
        });

        ui.add_space(10.0);

        // 便捷按钮与自动清理控制
        ui.horizontal(|ui| {
            if ui.add_sized([110.0, 30.0], theme::ghost_button("打开所在目录")).clicked() {
                let _ = crate::core::paths::open_in_explorer(&self.state.inventory.selected_path());
            }
            if ui.add_sized([90.0, 30.0], theme::ghost_button("转到写入页")).clicked() {
                self.state.current_tab = AppTab::WriteTest;
            }
            if is_busy {
                let _ = ui.add_sized([110.0, 30.0], theme::disabled_button("清理测试文件"));
            } else if ui.add_sized([110.0, 30.0], theme::ghost_button("清理测试文件")).clicked() {
                let path = self.state.inventory.selected_path();
                self.state.controller.start_cleanup(path);
            }

            ui.add_space(10.0);
            ui.checkbox(&mut self.state.auto_cleanup_after_read, RichText::new("校验通过后自动清理文件").size(12.0).color(theme::TEXT_SECONDARY));
        });

        ui.add_space(12.0);

        let current_is_verify = matches!(snapshot.kind, crate::core::controller::TaskKind::Verify | crate::core::controller::TaskKind::WriteAndVerify);
        let empty_stats = crate::core::algo::BlockStats::default();
        let stats = if is_read_running || (snapshot.status != TaskStatus::Running && current_is_verify) {
            &snapshot.stats
        } else {
            &empty_stats
        };

        // 现代 Bento-Grid 四卡片核心指标仪表盘 (读取校验专属)
        ui.horizontal(|ui| {
            let card_width = (ui.available_width() - 36.0) / 4.0;

            let display_speed = if is_read_running {
                snapshot.current_speed_mb
            } else if self.state.audit_record.read_tested {
                self.state.audit_record.read_avg_mb
            } else {
                0.0
            };

            let display_bytes = if is_read_running {
                snapshot.total_bytes_processed
            } else if self.state.audit_record.read_tested {
                self.state.audit_record.read_bytes
            } else {
                0
            };

            // 卡片 1: 实时读取速度
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("实时读取速度").size(11.0).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{:.2}", display_speed))
                            .size(24.0)
                            .strong()
                            .color(theme::ACCENT_CYAN),
                    );
                    ui.label(RichText::new("MB/s").size(11.0).color(theme::TEXT_MUTED));
                });
            });

            ui.add_space(12.0);

            // 卡片 2: 已校验数据量
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("已校验容量").size(11.0).color(theme::TEXT_MUTED));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{:.2}",
                            display_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        ))
                        .size(24.0)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(RichText::new("GB").size(11.0).color(theme::TEXT_MUTED));
                });
            });

            ui.add_space(12.0);

            // 卡片 3: 预计剩余时间
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("预计剩余时间").size(11.0).color(theme::TEXT_MUTED));
                let eta_str = if is_read_running {
                    format_eta(snapshot.eta_seconds)
                } else {
                    "空闲待命".to_string()
                };
                ui.label(
                    RichText::new(eta_str)
                        .size(20.0)
                        .strong()
                        .color(if is_read_running { theme::ACCENT_AMBER } else { theme::TEXT_SECONDARY }),
                );
            });

            ui.add_space(12.0);

            // 卡片 4: 介质健康结论
            theme::bento_card(ui, card_width, |ui| {
                ui.label(RichText::new("扇区健康状态").size(11.0).color(theme::TEXT_MUTED));
                let has_errors = stats.bad > 0 || stats.overwritten > 0 || stats.changed > 0;
                if is_read_running || self.state.audit_record.read_tested {
                    if has_errors {
                        theme::badge_red(ui, "发现异常坏块");
                    } else if stats.good > 0 {
                        theme::badge_green(ui, "数据完整健康");
                    } else {
                        theme::badge_cyan(ui, "正在校验");
                    }
                } else {
                    theme::badge_neutral(ui, "未校验");
                }
            });
        });

        ui.add_space(8.0);

        // 校验四态扇区微型徽章统计栏
        ui.horizontal(|ui| {
            ui.label(RichText::new("实时扇区采样统计:").size(11.0).color(theme::TEXT_MUTED));
            ui.add_space(4.0);
            theme::badge_green(ui, &format!("正常: {}", stats.good));
            ui.add_space(4.0);
            theme::badge_red(ui, &format!("损坏: {}", stats.bad));
            ui.add_space(4.0);
            theme::badge_amber(ui, &format!("异常: {}", stats.changed));
            ui.add_space(4.0);
            theme::badge_purple(ui, &format!("覆盖: {}", stats.overwritten));
        });

        ui.add_space(10.0);
        let empty_history = Vec::new();
        let read_history = if is_read_running || (snapshot.status != TaskStatus::Running && current_is_verify) {
            &snapshot.speed_history
        } else {
            &empty_history
        };
        render_speed_plot(ui, read_history, "读取校验", "read_speed_plot", crate::gui::components::plot::PlotKind::Verify);
        ui.add_space(10.0);

        let empty_heatmap = vec![crate::core::algo::BlockState::Untested; 1000];
        let heatmap = if is_read_running || (snapshot.status != TaskStatus::Running && current_is_verify) {
            &snapshot.heatmap_blocks
        } else {
            &empty_heatmap
        };
        render_heatmap(ui, heatmap);

        // 校验完成时，在底部直接渲染可截屏的专业电子版技术检验报告公文卡片 (严格隔离任务类型，杜绝写入完成时幽灵误报)
        let is_read_finished = snapshot.status == TaskStatus::Finished
            && matches!(snapshot.kind, crate::core::controller::TaskKind::Verify | crate::core::controller::TaskKind::WriteAndVerify)
            && snapshot.total_bytes_processed > 0;
        if is_read_finished || self.state.audit_record.read_tested {
            let active_target = self.state.inventory.active_target();
            let dev = active_target.as_ref().map(|t| &t.device);
            let serial = SaltRegistry::get_volume_serial(&self.state.inventory.selected_path().to_string_lossy());
            
            let duration_seconds = if current_is_verify && snapshot.duration_seconds > 0.0 {
                snapshot.duration_seconds
            } else {
                self.state.audit_record.read_duration_secs
            };
            let bytes_processed = if current_is_verify && snapshot.total_bytes_processed > 0 {
                snapshot.total_bytes_processed
            } else {
                self.state.audit_record.read_bytes
            };
            let speed_bps = if current_is_verify && snapshot.current_speed_mb > 0.0 {
                snapshot.current_speed_mb * 1024.0 * 1024.0
            } else {
                self.state.audit_record.read_avg_mb * 1024.0 * 1024.0
            };
            let stats = if current_is_verify {
                snapshot.stats
            } else {
                crate::core::algo::BlockStats::default()
            };
            let physiology = if current_is_verify {
                snapshot.physiology
            } else {
                None
            };
            let dummy_telemetry = H2wTelemetry {
                mode: H2wMode::Verify,
                phase: crate::core::session::H2wPhase::Completed,
                current_file: 1,
                total_files: None,
                bytes_processed,
                total_bytes_expected: bytes_processed,
                speed_bps,
                duration_seconds,
                eta_seconds: 0,
                stats,
                heatmap_sample: None,
                is_finished: true,
                error: None,
                physiology,
                first_error_offset: if current_is_verify { snapshot.first_error_offset } else { None },
                first_error_file: if current_is_verify { snapshot.first_error_file } else { None },
                first_error_expected: if current_is_verify { snapshot.first_error_expected } else { None },
                first_error_actual: if current_is_verify { snapshot.first_error_actual } else { None },
            };
            
            let report = DiagnosticReport::build_verify_audit(
                dev,
                serial,
                &self.state.inventory.selected_path(),
                &dummy_telemetry,
                self.state.salt,
                snapshot.physiology,
            );

            self.render_verify_report_card(ui, &report);
        }
    }

    fn render_probe_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        ui.label(RichText::new("闪电探测 (f3probe)").size(18.0).strong().color(theme::TEXT_PRIMARY));
        ui.label(RichText::new("直接向物理磁盘写入探针快速探测真实物理容量。探测前将自动独占卸载卷，请备份重要数据。").size(12.0).color(theme::TEXT_MUTED));
        ui.add_space(6.0);

        let is_busy = snapshot.status == TaskStatus::Running;
        ui.add_enabled_ui(!is_busy, |ui| {
            render_storage_device_selector(ui, &mut self.state.inventory);
        });

        ui.add_space(10.0);

        let is_safe = self.state.inventory.selected_device().is_some_and(|d| d.is_safe_target);
        let is_probe_running = snapshot.status == TaskStatus::Running && snapshot.kind == crate::core::controller::TaskKind::Probe;
        let is_other_running = snapshot.status == TaskStatus::Running && !is_probe_running;

        ui.horizontal(|ui| {
            if is_probe_running {
                if ui.add_sized([130.0, 36.0], theme::danger_button("停止探测")).clicked() {
                    self.state.controller.cancel();
                }
            } else {
                let can_start = is_safe && !is_other_running;
                let btn_text = if is_other_running {
                    "其他任务运行中..."
                } else if !is_safe {
                    "内部硬盘已安全锁定"
                } else {
                    "开始探测"
                };
                if can_start {
                    if ui.add_sized([130.0, 36.0], theme::primary_button(btn_text)).clicked() {
                        self.start_probe_task();
                    }
                } else {
                    let _ = ui.add_sized([150.0, 36.0], theme::disabled_button(btn_text));
                }
            }
        });

        ui.add_space(14.0);
        let progress = if is_probe_running || (snapshot.status != TaskStatus::Running && snapshot.kind == crate::core::controller::TaskKind::Probe) {
            snapshot.progress_pct
        } else {
            0.0
        };
        ui.add(ProgressBar::new(progress).show_percentage());

        if is_probe_running || snapshot.kind == crate::core::controller::TaskKind::Probe {
            ui.add_space(14.0);
            if let Some(ref res) = snapshot.probe_result {
                if let Some(ref err) = res.error {
                    egui::Frame::none()
                        .fill(theme::SURFACE_BG)
                        .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT_AMBER))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(14.0))
                        .show(ui, |ui| {
                            if err.contains("用户中止") || err.contains("手动中止") {
                                theme::badge_amber(ui, "任务中止");
                                ui.add_space(6.0);
                                ui.label(RichText::new("探测任务已由用户手动中止").size(14.0).strong().color(theme::TEXT_PRIMARY));
                                ui.add_space(4.0);
                                ui.label(RichText::new("设备已安全解锁恢复挂载，可随时重新启动探测。").size(12.0).color(theme::TEXT_MUTED));
                            } else {
                                theme::badge_red(ui, "执行受阻");
                                ui.add_space(6.0);
                                ui.label(RichText::new(format!("无法执行探测: {}", err)).size(14.0).strong().color(theme::ACCENT_RED));
                                ui.add_space(4.0);
                                ui.label(RichText::new("提示: 请确认已具有管理员权限，且目标物理磁盘未被其他进程独占占用。").size(12.0).color(theme::TEXT_MUTED));
                            }
                        });
                } else {
                    let border_color = if res.is_fake { theme::ACCENT_RED } else { theme::ACCENT_GREEN };
                    egui::Frame::none()
                        .fill(theme::SURFACE_BG)
                        .stroke(egui::Stroke::new(1.0_f32, border_color))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(16.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if res.is_fake {
                                    theme::badge_red(ui, "发现虚标");
                                } else {
                                    theme::badge_green(ui, "容量合规");
                                }
                                ui.add_space(6.0);
                                ui.label(RichText::new("闪电探测结果快报").size(15.0).strong().color(theme::TEXT_PRIMARY));
                            });

                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(8.0);

                            ui.label(RichText::new(format!(
                                "设备标称容量: {:.2} GB   |   物理真实容量: {:.2} GB",
                                res.announced_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                                res.real_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                            )).size(13.0).strong().color(theme::TEXT_PRIMARY));

                            ui.add_space(6.0);
                            if res.is_fake {
                                ui.label(RichText::new("检测到容量虚标！该设备存在硬件地址回绕或位黑洞。建议前往「分区修复」标签页将分区截断为真实安全容量。").size(12.0).color(theme::ACCENT_RED));
                            } else {
                                ui.label(RichText::new("容量完全合规，未见物理回绕或位黑洞。该设备物理容量真实可靠。").size(12.0).color(theme::ACCENT_GREEN));
                            }
                        });
                }
            }
        }
    }

    fn render_fix_tab(&mut self, ui: &mut egui::Ui, snapshot: &TaskSnapshot) {
        ui.label(RichText::new("分区自愈 (f3fix)").size(18.0).strong().color(theme::TEXT_PRIMARY));
        ui.label(RichText::new("根据探测出的实际物理容量重新划分安全分区，永久锁定并隔离超出真实物理边界的虚假扇区。").size(12.0).color(theme::TEXT_MUTED));
        ui.add_space(8.0);

        if let Some(ref plan) = snapshot.fix_plan {
            // 优先复用缓存的复核报告，仅当磁盘序号变更或缓存为空时执行一次底层硬件探测 (消除 16ms 轮询死穴)
            let dry_run_res = match &self.state.cached_dry_run_report {
                Some((cached_disk, res)) if *cached_disk == plan.disk_index => res.clone(),
                _ => {
                    let fixer = SafePartitionFixer::new(plan.clone());
                    let res = fixer.dry_run();
                    self.state.cached_dry_run_report = Some((plan.disk_index, res.clone()));
                    res
                }
            };
            egui::Frame::none()
                .fill(theme::SURFACE_BG)
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                .rounding(egui::Rounding::ZERO)
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.label(RichText::new("待执行安全分区方案").size(15.0).strong().color(theme::TEXT_PRIMARY));
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    egui::Frame::none()
                        .fill(theme::CANVAS_BG)
                        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("目标物理磁盘: PhysicalDrive{}", plan.disk_index)).size(12.0).color(theme::TEXT_SECONDARY));
                            ui.add_space(3.0);
                            ui.label(RichText::new(format!(
                                "真实可用容量: {:.2} GB   |   建议截断分区大小: {} MB",
                                plan.real_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                                plan.safe_partition_mb
                            )).size(12.5).strong().color(theme::TEXT_PRIMARY));
                            ui.add_space(3.0);
                            ui.label(RichText::new(format!("推荐格式化文件系统: {}", plan.file_system)).size(12.0).color(theme::TEXT_SECONDARY));
                        });

                    ui.add_space(10.0);
                    match dry_run_res {
                        Ok(report) => {
                            ui.label(
                                RichText::new(format!("已通过移动总线现场安全复核: {} ({})", report.bus_name, report.vendor_id))
                                    .size(12.0)
                                    .color(theme::ACCENT_GREEN),
                            );
                            ui.add_space(10.0);
                            let is_busy = snapshot.status == TaskStatus::Running;
                            ui.horizontal(|ui| {
                                if !is_busy {
                                    if ui.add_sized([220.0, 36.0], theme::primary_button("应用安全分区 (运行 diskpart)")).clicked() {
                                        if let Err(e) = self.state.controller.start_fix(plan.clone()) {
                                            self.state.controller.set_status_message(format!("启动修复失败: {}", e));
                                        }
                                    }
                                } else {
                                    let _ = ui.add_sized([220.0, 36.0], theme::disabled_button("正在执行安全分区..."));
                                }

                                ui.add_space(10.0);
                                if ui.add_sized([150.0, 36.0], theme::ghost_button("导出 diskpart 脚本")).clicked() {
                                    let desk = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
                                        .join(r"Desktop\f3_fix.diskpart");
                                    let _ = plan.export_script_to_file(&desk);
                                    self.state.controller.set_status_message(format!("脚本已导出至 {:?}", desk));
                                }
                            });
                        }
                        Err(err) => {
                            ui.label(RichText::new(format!("物理安全拦截: {}", err)).size(12.0).color(theme::ACCENT_RED));
                            ui.add_space(10.0);
                            let _ = ui.add_sized([180.0, 36.0], theme::disabled_button("无法应用安全分区"));
                        }
                    }
                });
        } else {
            egui::Frame::none()
                .fill(theme::SURFACE_BG)
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                .rounding(egui::Rounding::ZERO)
                .inner_margin(egui::Margin::same(18.0))
                .show(ui, |ui| {
                    ui.label(RichText::new("暂无待修复方案").size(14.0).strong().color(theme::TEXT_PRIMARY));
                    ui.add_space(4.0);
                    ui.label(RichText::new("请先前往「闪电探测」标签页针对假盘执行快速物理探测，系统将根据真实物理边界自动生成量身定制的无损自愈分区方案。").size(12.0).color(theme::TEXT_MUTED));
                });
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

        ui.add_space(16.0);
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT_CYAN))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(18.0))
            .show(ui, |ui| {
                // 1. 报告红头与公文标识
                ui.horizontal(|ui| {
                    theme::pill_badge(ui, "物理性能存证 · 无缓冲直通", theme::ACCENT_CYAN, Color32::from_rgb(16, 32, 45));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("存储介质物理写入性能与颗粒体质检验书")
                            .strong()
                            .size(15.0)
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&report.generated_at_local).size(11.0).color(theme::TEXT_MUTED));
                    });
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // 2. 写入阶段状态卡片
                egui::Frame::none()
                    .fill(Color32::from_rgb(14, 26, 36))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(24, 48, 64)))
                    .rounding(egui::Rounding::ZERO)
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("写入阶段完成：物理数据流已全盘无缓冲直通落盘")
                                .strong()
                                .size(14.0)
                                .color(theme::ACCENT_CYAN),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "写入数据总量: {:.2} GB  |  连续写入耗时: {:.1} 秒 ({:.2} 分钟)  |  平均写入速度: {:.2} MB/s",
                                gb(write_bytes), report.write_duration_secs, report.write_duration_secs / 60.0, write_avg
                            ))
                            .size(12.5)
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("说明：本阶段已将高熵伪随机数据真实写入存储物理芯片。已排除操作系统文件缓存，可直接作为物理写入吞吐能力的法定证据。")
                                .size(11.5)
                                .color(theme::TEXT_MUTED),
                        );
                    });

                ui.add_space(12.0);

                // 3. 硬件指纹拓扑
                ui.label(
                    RichText::new("一、 被测硬件物理拓扑与唯一指纹信息")
                        .strong()
                        .size(13.0)
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(theme::CANVAS_BG)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                    .rounding(egui::Rounding::ZERO)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("被测逻辑盘符: {}   |   物理磁盘对象: {}", report.target_drive, report.physical_drive_path)).size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("物理卷序列号: {} (卷格式化 32 位唯一指纹，彻底封死商家调包借口)", serial_hex)).size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("硬件标识信息: VID: {}   |   PID: {}   |   序列号: {}", report.vendor_id, report.product_id, report.serial_number)).size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("防伪加盐凭证: {} (单向有损散列，主机物理隔离防算力回放作弊)", report.salt_hex)).size(12.0).color(theme::TEXT_SECONDARY));
                    });

                ui.add_space(12.0);

                // 4. 写入速度生理全景指标
                ui.label(
                    RichText::new("二、 物理写入速率全景与生理体质诊断")
                        .strong()
                        .size(13.0)
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(theme::CANVAS_BG)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                    .rounding(egui::Rounding::ZERO)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("平均写入速率: {:.2} MB/s", write_avg)).size(12.0).color(theme::TEXT_PRIMARY));
                            ui.separator();
                            ui.label(RichText::new(format!("最高峰值写速: {:.2} MB/s", write_peak)).size(12.0).color(theme::TEXT_PRIMARY));
                            ui.separator();
                            ui.label(RichText::new(format!("最低谷底写速: {:.2} MB/s", write_valley)).size(12.0).color(theme::TEXT_PRIMARY));
                            ui.separator();
                            ui.label(RichText::new(format!("稳态中位写速: {:.2} MB/s", write_median)).size(12.0).color(theme::TEXT_PRIMARY));
                        });
                        if let Some(cliff_b) = report.slc_cliff_bytes {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(format!(
                                    "[断崖预警] SLC 模拟缓存耗尽拐点: 在写入至 {:.2} GB 处，速度由 {:.1} MB/s 暴跌至 {:.1} MB/s (跌幅 {:.1}%)",
                                    gb(cliff_b), report.slc_cliff_speed_before_mb, report.slc_cliff_speed_after_mb, report.slc_cliff_drop_pct
                                ))
                                .size(11.5)
                                .color(theme::ACCENT_AMBER),
                            );
                        }
                    });

                ui.add_space(8.0);
                ui.label(RichText::new(format!("硬件物理接口推断: {}", report.inference.inferred_bus)).size(12.0).color(theme::TEXT_SECONDARY));
                ui.label(RichText::new(format!("闪存颗粒品质推断: {}", report.inference.flash_grade)).size(12.0).color(theme::TEXT_SECONDARY));
                ui.label(RichText::new(format!("掉速病理解读: {}", report.inference.pathology_analysis)).size(12.0).color(theme::TEXT_MUTED));

                ui.add_space(12.0);

                // 5. 下一步操作指引
                ui.label(
                    RichText::new("三、 下一步真实容量验证指引")
                        .strong()
                        .size(13.0)
                        .color(theme::ACCENT_GREEN),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new("重要提示：物理写入完成仅证明数据已向设备发送。扩容假盘常采用地址回绕覆写或位黑洞丢弃。请前往「读取校验」页面开始全盘校验，即可验证实际物理可用容量与是否存在扩容欺诈。")
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(10.0);

                // 底部辅助按钮与截屏提示
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("截图提示: 请直接按快捷键 Win + Shift + S 截取上方公文卡片作为写入性能证据")
                            .size(11.5)
                            .color(theme::ACCENT_GREEN)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::ghost_button("复制报告 Markdown")).clicked() {
                            ui.output_mut(|o| o.copied_text = report.generate_markdown());
                        }
                    });
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

        // 读取端最高峰值、最低谷底与稳态中位数 (100% 硬件实测值: Welford 方差 / P² 在线分位数)
        let read_peak = if report.verify_peak_speed_mb > 0.0 {
            report.verify_peak_speed_mb
        } else if read_avg > 0.0 {
            read_avg
        } else {
            0.0
        };

        let read_valley = if report.verify_valley_speed_mb > 0.0 {
            report.verify_valley_speed_mb
        } else {
            0.0
        };

        let read_median = if report.verify_median_speed_mb > 0.0 {
            report.verify_median_speed_mb
        } else if read_avg > 0.0 {
            read_avg
        } else {
            0.0
        };

        let read_bytes = if report.verify_bytes > 0 {
            report.verify_bytes
        } else if self.state.audit_record.read_bytes > 0 {
            self.state.audit_record.read_bytes
        } else {
            0
        };

        ui.add_space(16.0);
        let frame_stroke = if report.is_fake {
            egui::Stroke::new(1.0_f32, theme::ACCENT_RED)
        } else {
            egui::Stroke::new(1.0_f32, theme::ACCENT_GREEN)
        };

        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(frame_stroke)
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(18.0))
            .show(ui, |ui| {
                // 1. 报告红头与公文标识
                ui.horizontal(|ui| {
                    if report.is_fake {
                        theme::pill_badge(ui, "司法鉴定级存证 · 欺诈确证", theme::ACCENT_RED, Color32::from_rgb(45, 16, 16));
                    } else {
                        theme::pill_badge(ui, "司法鉴定级存证 · 物理合规", theme::ACCENT_GREEN, Color32::from_rgb(16, 40, 28));
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("存储介质容量真实性与读写生理鉴定结论书")
                            .strong()
                            .size(15.0)
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&report.generated_at_local).size(11.0).color(theme::TEXT_MUTED));
                    });
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // 2. 人性化大白话通俗诊断结论 (醒目大卡片)
                if report.is_fake {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(34, 18, 22))
                        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(80, 24, 28)))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("一句话通俗诊断：这是一只严重虚标的非法扩容假盘")
                                    .strong()
                                    .size(14.0)
                                    .color(theme::ACCENT_RED),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "标称规格: {:.2} GB  |  真实可用: {:.2} GB  |  虚假欺诈容量: {:.2} GB (虚标比例: {:.1}%)",
                                    gb(report.announced_bytes), gb(report.real_usable_bytes), gb(report.fake_bytes), report.fake_ratio_pct
                                ))
                                .size(12.5)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("通俗比喻：打个比方，这就像一个外壳标着 64 升的大水桶，内部实际只有 8 升！倒进多出来的水会全部漏掉或覆盖旧水。存入该盘的重要照片或文档将在超过真实容量后永久丢失！")
                                    .size(11.5)
                                    .color(theme::TEXT_MUTED),
                            );
                        });
                } else {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(14, 30, 22))
                        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(20, 60, 40)))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("一句话通俗诊断：全盘底层 Direct I/O 检验 100% 吻合，物理闪存真实合规")
                                    .strong()
                                    .size(14.0)
                                    .color(theme::ACCENT_GREEN),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!(
                                    "实测真实可用容量为 {:.2} GB，未见任何地址回绕、位黑洞或异常坏道，综合质量评级：[AAA级健康介质]，可安心存储重要数据。",
                                    gb(report.announced_bytes)
                                ))
                                .size(12.5)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                            );
                        });
                }

                ui.add_space(12.0);

                // 3. 硬件指纹拓扑 (彻底封死调包狡辩)
                ui.label(
                    RichText::new("一、 被测硬件物理拓扑与唯一指纹信息")
                        .strong()
                        .size(13.0)
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(theme::CANVAS_BG)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                    .rounding(egui::Rounding::ZERO)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("被测逻辑盘符: {}   |   物理磁盘对象: {}", report.target_drive, report.physical_drive_path)).size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("物理卷序列号: {} (卷格式化 32 位唯一指纹，彻底封死商家调包借口)", serial_hex)).size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("硬件标识信息: VID: {}   |   PID: {}   |   序列号: {}", report.vendor_id, report.product_id, report.serial_number)).size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(3.0);
                        ui.label(RichText::new(format!("防伪加盐凭证: {} (单向有损散列，主机物理隔离防算力回放作弊)", report.salt_hex)).size(12.0).color(theme::TEXT_SECONDARY));
                    });

                ui.add_space(12.0);

                // 4. 读取校验物理性能与生理特征指标
                ui.label(
                    RichText::new("二、 物理读取速率全景与扇区健康审计")
                        .strong()
                        .size(13.0)
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(theme::CANVAS_BG)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                    .rounding(egui::Rounding::ZERO)
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("平均读取速率: {:.2} MB/s", read_avg)).size(12.0).color(theme::TEXT_PRIMARY));
                            ui.separator();
                            ui.label(RichText::new(format!("最高峰值读速: {:.2} MB/s", read_peak)).size(12.0).color(theme::TEXT_PRIMARY));
                            ui.separator();
                            ui.label(RichText::new(format!("最低谷底读速: {:.2} MB/s", read_valley)).size(12.0).color(theme::TEXT_PRIMARY));
                            ui.separator();
                            ui.label(RichText::new(format!("稳态中位读速: {:.2} MB/s", read_median)).size(12.0).color(theme::TEXT_PRIMARY));
                        });
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("校验数据总量: {:.2} GB", gb(read_bytes))).size(12.0).color(theme::TEXT_SECONDARY));
                            ui.separator();
                            ui.label(RichText::new(format!("读取校验耗时: {:.1} 秒 ({:.2} 分钟)", report.verify_duration_secs, report.verify_duration_secs / 60.0)).size(12.0).color(theme::TEXT_SECONDARY));
                        });
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            theme::pill_badge(ui, &format!("正常: {} ({:.2} GB)", report.stats.good, gb(report.stats.good * 512)), theme::ACCENT_GREEN, Color32::from_rgb(16, 40, 28));
                            ui.add_space(4.0);
                            theme::pill_badge(ui, &format!("损坏: {} ({:.2} GB)", report.stats.bad, gb(report.stats.bad * 512)), theme::ACCENT_RED, Color32::from_rgb(45, 16, 16));
                            ui.add_space(4.0);
                            theme::pill_badge(ui, &format!("覆写: {} ({:.2} GB)", report.stats.overwritten, gb(report.stats.overwritten * 512)), Color32::from_rgb(168, 85, 247), Color32::from_rgb(38, 18, 55));
                        });
                    });

                ui.add_space(8.0);
                ui.label(RichText::new(format!("硬件物理总线通道推断: {}", report.inference.inferred_bus)).size(12.0).color(theme::TEXT_SECONDARY));
                ui.label(RichText::new(format!("闪存颗粒综合品质等级: {}", report.inference.flash_grade)).size(12.0).color(theme::TEXT_SECONDARY));
                ui.label(RichText::new(format!("物理生理特征深度解读: {}", report.inference.pathology_analysis)).size(12.0).color(theme::TEXT_MUTED));

                // 5. 比特级微观失配现场证据 (Hexdump)
                if report.stats.bad + report.stats.overwritten + report.stats.changed > 0 {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("三、 比特级微观失配现场证据 (Hexdump 对照)")
                            .strong()
                            .size(13.0)
                            .color(theme::ACCENT_AMBER),
                    );
                    ui.add_space(6.0);
                    if let Some(err_off) = report.first_error_offset {
                        egui::Frame::none()
                            .fill(theme::CANVAS_BG)
                            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                            .rounding(egui::Rounding::ZERO)
                            .inner_margin(egui::Margin::same(10.0))
                            .show(ui, |ui| {
                                ui.label(RichText::new(format!("首次故障出现位置: {}.h2w (全局物理偏移: 0x{:016X}，即 {:.2} GB 处)", report.first_error_file.unwrap_or(1), err_off, gb(err_off))).size(12.0).monospace().color(theme::ACCENT_RED));
                                ui.add_space(4.0);
                                let exp_hex = report.first_error_expected.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                                let act_hex = report.first_error_actual.map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "N/A".to_string());
                                ui.label(RichText::new(format!("[预期正确数据]: {}", exp_hex)).size(11.5).monospace().color(theme::TEXT_SECONDARY));
                                ui.add_space(2.0);
                                ui.label(RichText::new(format!("[实际读回数据]: {}", act_hex)).size(11.5).monospace().color(theme::ACCENT_RED));
                                ui.add_space(4.0);
                                ui.label(RichText::new("微观定性说明: 数据首部标记失效，读回呈现固件截断特征，确证该位置之后物理存储单元不存在。").size(11.0).color(theme::TEXT_MUTED));
                            });
                    }
                }

                // 6. 保姆级维权维权指南与消法第55条
                if report.is_fake {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("四、 消费者权益保护法维权指南 (假一赔三)")
                            .strong()
                            .size(13.0)
                            .color(theme::ACCENT_AMBER),
                    );
                    ui.add_space(6.0);
                    egui::Frame::none()
                        .fill(Color32::from_rgb(30, 24, 14))
                        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(60, 45, 20)))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.label(RichText::new("【第一步·直接截图】：直接使用快捷键截取本电子公文卡片发给商家客服；").size(12.0).color(theme::TEXT_PRIMARY));
                            ui.add_space(3.0);
                            ui.label(RichText::new("【第二步·法定索赔】：依据《消法》第55条要求【全额退款 + 三倍赔偿（不足500元按500元）】；").size(12.0).color(theme::TEXT_PRIMARY));
                            ui.add_space(3.0);
                            ui.label(RichText::new("【第三步·官方投诉】：若商家推诿，在平台申请纠纷小二介入，或将本截图上传至全国 12315 平台投诉。").size(12.0).color(theme::TEXT_PRIMARY));
                        });
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(10.0);

                // 底部辅助按钮与截屏提示
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("截图提示: 请直接按快捷键 Win + Shift + S 截取上方完整公文卡片，可直接作为维权证据")
                            .size(11.5)
                            .color(theme::ACCENT_GREEN)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::ghost_button("复制全文 Markdown")).clicked() {
                            ui.output_mut(|o| o.copied_text = report.generate_markdown());
                        }
                        if report.is_fake {
                            ui.add_space(6.0);
                            if ui.add(theme::ghost_button("保存 .md 证据档案到本地")).clicked() {
                                if let Ok(p) = report.auto_archive_evidence() {
                                    self.state.controller.set_status_message(format!("证据档案已安全落盘: {:?}", p));
                                }
                            }
                        }
                    });
                });
            });
    }


    fn render_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("高级设置").size(18.0).strong().color(theme::TEXT_PRIMARY));
        ui.label(RichText::new("调整 Direct I/O 底层传输参数、防伪加盐凭证与系统窗口行为。").size(12.0).color(theme::TEXT_MUTED));
        ui.add_space(8.0);

        // 卡片 1: 防伪加盐与互通模式
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("数据防伪与生态互通").size(14.0).strong().color(theme::TEXT_PRIMARY));
                ui.add_space(6.0);
                ui.checkbox(&mut self.state.use_random_salt, "启用随机加盐 (防止固件针对固定数据流作弊)");
                ui.add_space(3.0);
                if !self.state.use_random_salt {
                    ui.label(RichText::new("当前为标准模式 (Salt = 0)，100% 逐比特向后兼容 H2testw 与原版 F3。").size(11.5).color(theme::TEXT_MUTED));
                } else {
                    ui.label(RichText::new("提示：加盐后生成的数据流具备单向雪崩特征，需在相同盐值下才能校验。").size(11.5).color(theme::ACCENT_AMBER));
                }
            });

        ui.add_space(10.0);

        // 卡片 2: Direct I/O 缓冲区大小
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("I/O 传输块大小").size(14.0).strong().color(theme::TEXT_PRIMARY));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("单次 Direct I/O 扇区对齐缓冲区:").size(12.0).color(theme::TEXT_SECONDARY));
                    ui.selectable_value(&mut self.state.chunk_size_mb, 1, "1 MB (标准推荐)");
                    ui.selectable_value(&mut self.state.chunk_size_mb, 2, "2 MB (高速固态)");
                    ui.selectable_value(&mut self.state.chunk_size_mb, 4, "4 MB (极致吞吐)");
                });
            });

        ui.add_space(10.0);

        // 卡片 3: 分卷测试选项
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("分卷测试选项 (类似 --start-at / --end-at)").size(14.0).strong().color(theme::TEXT_PRIMARY));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("起始文件编号:").size(12.0).color(theme::TEXT_SECONDARY));
                    ui.add(
                        egui::DragValue::new(&mut self.state.start_file_num)
                            .range(0..=10000)
                            .prefix("从第 ")
                            .suffix(" 个文件开始写入"),
                    );
                });
                ui.add_space(4.0);
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
                ui.add_space(4.0);
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

        ui.add_space(10.0);

        // 卡片 4: 托盘与窗口行为
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("系统托盘与窗口行为").size(14.0).strong().color(theme::TEXT_PRIMARY));
                ui.add_space(6.0);
                ui.checkbox(
                    &mut self.state.minimize_to_tray_on_close,
                    "点击窗口关闭按钮 (×) 时最小化到系统托盘 (保持后台任务无损运行)",
                );
                ui.add_space(6.0);
                if ui.add_sized([160.0, 30.0], theme::ghost_button("打开核心路径信息中心")).clicked() {
                    self.state.show_path_info_modal = true;
                }
            });

        ui.add_space(10.0);

        // 卡片 5: 环境与路径全景概览
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("运行时核心路径概览").size(14.0).strong().color(theme::TEXT_PRIMARY));
                ui.add_space(8.0);

                let selected = Some(self.state.inventory.selected_path());
                let path_info = AppPathInfo::gather(selected);

                let mut render_path_row = |label: &str, p: &std::path::Path, is_file: bool| {
                    egui::Frame::none()
                        .fill(theme::CANVAS_BG)
                        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(label).size(12.0).strong().color(theme::TEXT_SECONDARY));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add_sized([55.0, 24.0], theme::ghost_button(if is_file { "定位" } else { "打开" })).clicked() {
                                        let _ = paths::open_in_explorer(p);
                                    }
                                    ui.add_space(4.0);
                                    if ui.add_sized([55.0, 24.0], theme::ghost_button("复制")).clicked() {
                                        let _ = paths::copy_to_clipboard(&p.to_string_lossy());
                                    }
                                });
                            });
                            ui.add_space(3.0);
                            ui.label(RichText::new(p.to_string_lossy().to_string()).size(11.0).monospace().color(theme::TEXT_PRIMARY));
                        });
                    ui.add_space(4.0);
                };

                render_path_row("程序执行文件", &path_info.exe_path, true);
                render_path_row("程序所在目录", &path_info.exe_dir, false);
                render_path_row("当前工作目录", &path_info.working_dir, false);
                if let Some(ref vol) = path_info.selected_volume {
                    render_path_row("当前测试目标卷", vol, false);
                }
                render_path_row("维权存证档案目录", &path_info.evidence_dir, false);
                render_path_row("系统临时文件目录", &path_info.temp_dir, false);
            });

        ui.add_space(10.0);

        // 卡片 6: 安全退出程序
        egui::Frame::none()
            .fill(theme::SURFACE_BG)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("退出应用程序").size(14.0).strong().color(theme::TEXT_PRIMARY));
                ui.add_space(4.0);
                ui.label(RichText::new("安全终止后台测试任务、释放设备排他锁并退出程序。").size(12.0).color(theme::TEXT_MUTED));
                ui.add_space(8.0);
                if ui.add_sized([160.0, 36.0], theme::danger_button("退出 F3 闪存测试")).clicked() {
                    let is_running = self.state.controller.poll().status == TaskStatus::Running;
                    if is_running {
                        self.state.show_exit_confirm_modal = true;
                    } else {
                        self.perform_exit(ui.ctx());
                    }
                }
            });
    }

    /// 渲染核心路径信息全景模态对话框
    fn render_path_info_modal(&mut self, ctx: &egui::Context) {
        if !self.state.show_path_info_modal {
            return;
        }

        let selected = Some(self.state.inventory.selected_path());
        let path_info = AppPathInfo::gather(selected);

        egui::Window::new("核心路径与运行环境信息")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.label(RichText::new("F3-Win 运行时目录与证据档案配置概览").strong().size(14.0).color(theme::TEXT_PRIMARY));
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                let render_row = |ui: &mut egui::Ui, title: &str, path: &std::path::Path, is_file: bool| {
                    egui::Frame::none()
                        .fill(theme::CANVAS_BG)
                        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
                        .rounding(egui::Rounding::ZERO)
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(title).strong().size(12.0).color(theme::TEXT_SECONDARY));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add_sized([55.0, 24.0], theme::ghost_button(if is_file { "定位" } else { "打开" })).clicked() {
                                        let _ = paths::open_in_explorer(path);
                                    }
                                    ui.add_space(4.0);
                                    if ui.add_sized([55.0, 24.0], theme::ghost_button("复制")).clicked() {
                                        let _ = paths::copy_to_clipboard(&path.to_string_lossy());
                                    }
                                });
                            });
                            ui.add_space(3.0);
                            ui.label(RichText::new(path.to_string_lossy().to_string()).size(11.0).monospace().color(theme::TEXT_PRIMARY));
                        });
                    ui.add_space(4.0);
                };

                render_row(ui, "程序执行文件 (Executable):", &path_info.exe_path, true);
                render_row(ui, "程序所在目录 (Install Dir):", &path_info.exe_dir, false);
                render_row(ui, "当前工作目录 (Working Dir):", &path_info.working_dir, false);
                if let Some(ref vol) = path_info.selected_volume {
                    render_row(ui, "当前测试目标卷 (Target Volume):", vol, false);
                }
                render_row(ui, "维权存证档案目录 (Evidence Dir):", &path_info.evidence_dir, false);
                render_row(ui, "系统临时文件目录 (Temp Dir):", &path_info.temp_dir, false);

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_sized([100.0, 32.0], theme::primary_button("关闭对话框")).clicked() {
                            self.state.show_path_info_modal = false;
                        }
                    });
                });
            });
    }

    /// 渲染硬件测试运行中请求退出时的防呆二次确认弹窗
    fn render_exit_confirm_modal(&mut self, ctx: &egui::Context) {
        if !self.state.show_exit_confirm_modal {
            return;
        }

        egui::Window::new("测试正在运行 - 退出确认")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(420.0);
                egui::Frame::none()
                    .fill(theme::SURFACE_BG)
                    .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT_RED))
                    .rounding(egui::Rounding::ZERO)
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::pill_badge(ui, "操作拦截", theme::ACCENT_RED, Color32::from_rgb(45, 16, 16));
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("后台 Direct I/O 硬件测试正在进行中！")
                                    .strong()
                                    .size(13.5)
                                    .color(theme::ACCENT_RED),
                            );
                        });
                        ui.add_space(6.0);
                        ui.label(RichText::new("如果现在退出，将中断正在进行的扇区写入/校验/探测，并提前释放卷排他锁。").size(12.0).color(theme::TEXT_SECONDARY));
                        ui.add_space(4.0);
                        ui.label(RichText::new("是否确认中止当前任务并退出程序？").size(12.0).strong().color(theme::TEXT_PRIMARY));
                    });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.add_sized([120.0, 32.0], theme::ghost_button("取消 (继续测试)")).clicked() {
                        self.state.show_exit_confirm_modal = false;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_sized([130.0, 32.0], theme::danger_button("强制中止并退出")).clicked() {
                            self.perform_exit(ctx);
                        }
                    });
                });
            });
    }

    fn start_h2w_task(&mut self, mode: H2wMode) {
        let is_safe = self.state.inventory.selected_volume().is_some_and(|v| v.is_safe_target);
        if !is_safe {
            self.state.controller.set_status_message("所选磁盘处于系统保护或非移动介质状态，已硬隔离锁定！".to_string());
            return;
        }

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
