use std::path::PathBuf;
use eframe::egui::{self, Color32, ProgressBar, RichText};

use crate::core::controller::{TaskKind, TaskSnapshot, TaskStatus};
use crate::core::fix::SafePartitionFixer;
use crate::core::session::{H2wConfig, H2wMode, H2wSession};
use crate::gui::components::device_selector::{
    render_storage_device_selector, render_storage_volume_selector,
};
use crate::gui::components::heatmap::render_heatmap;
use crate::gui::components::plot::render_speed_plot;
use crate::gui::state::{AppState, AppTab};

pub struct F3App {
    state: AppState,
}

impl F3App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::setup_chinese_fonts(&cc.egui_ctx);
        Self {
            state: AppState::default(),
        }
    }
}

impl eframe::App for F3App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 单行获取深层任务控制器轮询出的最新只读快照
        let snapshot = self.state.controller.poll().clone();
        ctx.request_repaint(); // 保证动画与采样平滑

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
            if ui.button(RichText::new("清理测试文件 (.h2w)")).clicked() {
                match H2wSession::cleanup_h2w_files(&self.state.inventory.selected_path()) {
                    Ok((cnt, bytes)) => {
                        self.state.controller.set_status_message(format!(
                            "已成功清理 {} 个文件，释放 {:.2} GB 磁盘空间！",
                            cnt,
                            bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        ));
                        self.state.inventory.refresh();
                    }
                    Err(e) => self.state.controller.set_status_message(format!("清理失败: {}", e)),
                }
            }
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &snapshot.speed_history, "写入", "write_speed_plot");
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
            if ui
                .button(
                    RichText::new("一键清理所有 .h2w 测试文件 (释放空间)")
                        .color(Color32::from_rgb(255, 140, 0)),
                )
                .clicked()
            {
                match H2wSession::cleanup_h2w_files(&self.state.inventory.selected_path()) {
                    Ok((cnt, bytes)) => {
                        self.state.controller.set_status_message(format!(
                            "已成功清理 {} 个文件，释放 {:.2} GB 磁盘空间！",
                            cnt,
                            bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        ));
                        self.state.inventory.refresh();
                    }
                    Err(e) => self.state.controller.set_status_message(format!("清理失败: {}", e)),
                }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.state.auto_cleanup_after_read, "校验完成后自动删除测试文件");
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &snapshot.speed_history, "读取", "read_speed_plot");
        ui.add_space(10.0);
        render_heatmap(ui, &snapshot.heatmap_blocks);

        // 测试报告导出面板
        if snapshot.status == TaskStatus::Finished && snapshot.stats.good + snapshot.stats.bad > 0 {
            ui.add_space(10.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.heading("测试报告:");
                    if ui.button(RichText::new("复制测试报告到剪贴板")).clicked() {
                        let report = format!(
                            "# F3-Win 闪存检测诊断报告\n\
                            - 测试路径: {:?}\n\
                            - 平均读速: {:.2} MB/s\n\
                            - 健康扇区 (Good): {}\n\
                            - 损毁扇区 (Bad): {}\n\
                            - 变异扇区 (Changed): {}\n\
                            - 回绕覆写 (Overwritten): {}\n\
                            - 诊断结论: {}\n",
                            self.state.inventory.selected_path(),
                            snapshot.current_speed_mb,
                            snapshot.stats.good,
                            snapshot.stats.bad,
                            snapshot.stats.changed,
                            snapshot.stats.overwritten,
                            if snapshot.stats.overwritten > 0 {
                                "确认为虚标扩容假盘！"
                            } else if snapshot.stats.bad > 0 {
                                "存在闪存物理坏道！"
                            } else {
                                "全部扇区健康合规！"
                            }
                        );
                        ui.output_mut(|o| o.copied_text = report);
                    }
                });
            });
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
                    ui.colored_label(
                        Color32::RED,
                        RichText::new(format!("无法执行探测: {}", err)).strong().size(15.0),
                    );
                    ui.label("提示: 请确认已允许管理员权限，且设备未被其他程序占用。");
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
                        if ui.button(RichText::new("应用安全分区 (运行 diskpart)").size(15.0)).clicked() {
                            match fixer.execute() {
                                Ok(msg) => {
                                    self.state.controller.set_status_message(format!("修复成功: {}", msg));
                                    self.state.inventory.refresh();
                                }
                                Err(e) => self.state.controller.set_status_message(format!("修复失败: {}", e)),
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
        let config = H2wConfig {
            target_dir: self.state.inventory.selected_path(),
            mode,
            start_file_num: self.state.start_file_num,
            end_file_num: self.state.end_file_num,
            max_bytes: self.state.max_test_gb.map(|gb| gb * 1024 * 1024 * 1024),
            chunk_size: self.state.chunk_size_mb * 1024 * 1024,
            salt: if self.state.use_random_salt {
                use std::time::SystemTime;
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0x5A5A_A5A5_1234_5678)
            } else {
                self.state.salt
            },
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
