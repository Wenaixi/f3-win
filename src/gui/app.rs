use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;
use crossbeam_channel::unbounded;
use eframe::egui::{self, Color32, ProgressBar, RichText};

use crate::core::devices::{PhysicalDeviceInfo, VolumeInfo};
use crate::core::fix::F3FixPlan;
use crate::core::probe::{F3Probe, F3ProbeConfig};
use crate::core::reader::{F3Reader, F3ReaderConfig};
use crate::core::writer::{F3Writer, F3WriterConfig};
use crate::gui::components::device_selector::{render_device_selector, render_volume_selector};
use crate::gui::components::heatmap::render_heatmap;
use crate::gui::components::plot::render_speed_plot;
use crate::gui::state::{AppState, AppTab, TaskStatus};

pub struct F3App {
    state: AppState,
    start_instant: Option<Instant>,
}

impl F3App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::setup_chinese_fonts(&cc.egui_ctx);
        let mut state = AppState::default();
        state.devices = PhysicalDeviceInfo::enumerate_devices();
        state.selected_device = state.devices.iter().find(|d| d.is_safe_target).cloned();
        state.volumes = VolumeInfo::enumerate_volumes();
        if let Some(first_removable) = state.volumes.iter().position(|v| v.is_removable) {
            state.selected_volume_index = first_removable;
            state.selected_path = state.volumes[first_removable].root_path.clone();
        } else if !state.volumes.is_empty() {
            state.selected_volume_index = 0;
            state.selected_path = state.volumes[0].root_path.clone();
        }

        Self {
            state,
            start_instant: None,
        }
    }

    fn poll_telemetry(&mut self) {
        // 1. 轮询写入遥测
        if let Some(ref rx) = self.state.write_rx {
            while let Ok(msg) = rx.try_recv() {
                self.state.total_bytes_processed = msg.total_bytes_written;
                self.state.current_speed_mb = msg.current_speed_bps / (1024.0 * 1024.0);
                self.state.eta_seconds = msg.eta_seconds;

                if let Some(start) = self.start_instant {
                    let elapsed = start.elapsed().as_secs_f64();
                    self.state.speed_history.push([elapsed, self.state.current_speed_mb]);
                }

                if msg.is_finished {
                    self.state.status = if msg.error.is_some() {
                        TaskStatus::Error
                    } else {
                        TaskStatus::Finished
                    };
                    self.state.status_message = msg.error.unwrap_or_else(|| "写入测试完成".to_string());
                    self.state.write_rx = None;
                    break;
                }
            }
        }

        // 2. 轮询读取遥测
        if let Some(ref rx) = self.state.read_rx {
            while let Ok(msg) = rx.try_recv() {
                self.state.total_bytes_processed = msg.total_bytes_read;
                self.state.current_speed_mb = msg.current_speed_bps / (1024.0 * 1024.0);
                self.state.stats = msg.stats;

                if let Some((idx, state_val)) = msg.heatmap_sample {
                    if idx < self.state.heatmap_blocks.len() {
                        self.state.heatmap_blocks[idx] = state_val;
                    }
                }

                if let Some(start) = self.start_instant {
                    let elapsed = start.elapsed().as_secs_f64();
                    self.state.speed_history.push([elapsed, self.state.current_speed_mb]);
                }

                if msg.is_finished {
                    self.state.status = if msg.error.is_some() {
                        TaskStatus::Error
                    } else {
                        TaskStatus::Finished
                    };
                    self.state.status_message = msg.error.unwrap_or_else(|| "校验测试完成".to_string());
                    self.state.read_rx = None;
                    break;
                }
            }
        }

        // 3. 轮询探针探测遥测
        if let Some(ref rx) = self.state.probe_rx {
            while let Ok(msg) = rx.try_recv() {
                self.state.progress_pct = msg.progress_pct;
                self.state.status_message = msg.current_phase.clone();

                if msg.is_finished {
                    self.state.status = if msg.error.is_some() {
                        TaskStatus::Error
                    } else {
                        TaskStatus::Finished
                    };
                    let is_fake = msg.is_fake;
                    let announced = msg.announced_size_bytes;
                    let real = msg.real_size_bytes;
                    self.state.probe_result = Some(msg);

                    if is_fake {
                        if let Some(ref dev) = self.state.selected_device {
                            self.state.fix_plan = Some(F3FixPlan::new(dev.drive_index, announced, real));
                        }
                    }
                    self.state.probe_rx = None;
                    break;
                }
            }
        }
    }
}

impl eframe::App for F3App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_telemetry();
        ctx.request_repaint(); // 保证动画与采样平滑

        egui::TopBottomPanel::top("top_nav").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("F3-Win 极致闪存测试与自愈终端").strong().color(Color32::from_rgb(0, 200, 255)));
                ui.separator();

                ui.selectable_value(&mut self.state.current_tab, AppTab::WriteTest, "全盘写入 (f3write)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::ReadVerify, "全盘读取 (f3read)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::RapidProbe, "闪电探测 (f3probe)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::PartitionFix, "分区自愈 (f3fix)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::Settings, "防作弊设置");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.state.current_tab {
                AppTab::WriteTest => self.render_write_tab(ui),
                AppTab::ReadVerify => self.render_read_tab(ui),
                AppTab::RapidProbe => self.render_probe_tab(ui),
                AppTab::PartitionFix => self.render_fix_tab(ui),
                AppTab::Settings => self.render_settings_tab(ui),
            }
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("状态: {}", self.state.status_message));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("Direct I/O 无缓冲模式: 已激活");
                });
            });
        });
    }
}

impl F3App {
    fn render_write_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("文件系统级写入测试 (f3write)");
        ui.label("在指定磁盘空闲空间中写入标准 .h2w 数据文件，直通写入并测试极限物理写速。");
        ui.separator();

        let mut do_refresh = false;
        render_volume_selector(
            ui,
            "write_vol_combo",
            &self.state.volumes,
            &mut self.state.selected_volume_index,
            &mut self.state.selected_path,
            || { do_refresh = true; }
        );
        if do_refresh {
            self.state.volumes = VolumeInfo::enumerate_volumes();
        }

        ui.add_space(10.0);

        if self.state.status != TaskStatus::Running {
            if ui.button(RichText::new("开始极速写入测试").size(16.0).color(Color32::GREEN)).clicked() {
                self.start_write_task();
            }
        } else {
            if ui.button(RichText::new("停止写入").size(16.0).color(Color32::RED)).clicked() {
                self.state.cancel_flag.store(true, Ordering::Relaxed);
            }
        }

        ui.add_space(10.0);

        // 现代 Bento-Grid 四卡片核心指标仪表盘
        ui.horizontal(|ui| {
            let card_width = (ui.available_width() - 30.0) / 4.0;
            
            // 卡片 1: 瞬时速率
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("瞬时写入速率").weak().size(12.0));
                ui.label(RichText::new(format!("{:.2} MB/s", self.state.current_speed_mb)).strong().size(20.0).color(Color32::from_rgb(0, 200, 255)));
            });

            // 卡片 2: 已写入数据
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("累计已写入容量").weak().size(12.0));
                ui.label(RichText::new(format!("{:.2} GB", self.state.total_bytes_processed as f64 / (1024.0 * 1024.0 * 1024.0))).strong().size(20.0));
            });

            // 卡片 3: 动态 ETA 倒计时
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("预计剩余时间 (ETA)").weak().size(12.0));
                let eta_str = if self.state.status == TaskStatus::Running {
                    format_eta(self.state.eta_seconds)
                } else {
                    "空闲就绪".to_string()
                };
                ui.label(RichText::new(eta_str).strong().size(20.0).color(Color32::from_rgb(255, 180, 0)));
            });

            // 卡片 4: I/O 驱动模式
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("底层 I/O 模式").weak().size(12.0));
                ui.label(RichText::new("Direct I/O 直通").strong().size(20.0).color(Color32::GREEN));
            });
        });

        // 便捷按钮: 在资源管理器中打开当前测试盘
        ui.horizontal(|ui| {
            if ui.button("在文件资源管理器中打开此路径").clicked() {
                let _ = std::process::Command::new("explorer").arg(&self.state.selected_path).spawn();
            }
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &self.state.speed_history, "写入", "write_speed_plot");
    }

    fn render_read_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("全盘读取与坏块校验 (f3read)");
        ui.label("回读现存的 .h2w 文件，逐扇区比对 LCG 序列，发现虚标回绕与闪存物理坏道。");
        ui.separator();

        let mut do_refresh = false;
        render_volume_selector(
            ui,
            "read_vol_combo",
            &self.state.volumes,
            &mut self.state.selected_volume_index,
            &mut self.state.selected_path,
            || { do_refresh = true; }
        );
        if do_refresh {
            self.state.volumes = VolumeInfo::enumerate_volumes();
        }

        ui.add_space(10.0);

        if self.state.status != TaskStatus::Running {
            if ui.button(RichText::new("开始读取校验").size(16.0).color(Color32::GREEN)).clicked() {
                self.start_read_task();
            }
        } else {
            if ui.button(RichText::new("停止读取").size(16.0).color(Color32::RED)).clicked() {
                self.state.cancel_flag.store(true, Ordering::Relaxed);
            }
        }

        ui.add_space(10.0);

        // 校验统计计数
        ui.horizontal(|ui| {
            ui.colored_label(Color32::from_rgb(46, 204, 113), format!("健康 (Good): {}", self.state.stats.good));
            ui.separator();
            ui.colored_label(Color32::from_rgb(231, 76, 60), format!("损坏 (Bad): {}", self.state.stats.bad));
            ui.separator();
            ui.colored_label(Color32::from_rgb(241, 196, 15), format!("变异 (Changed): {}", self.state.stats.changed));
            ui.separator();
            ui.colored_label(Color32::from_rgb(155, 89, 182), format!("回绕覆写 (Overwritten): {}", self.state.stats.overwritten));
        });

        // 便捷按钮
        ui.horizontal(|ui| {
            if ui.button("在文件资源管理器中打开此路径").clicked() {
                let _ = std::process::Command::new("explorer").arg(&self.state.selected_path).spawn();
            }
            if ui.button("前往「全盘写入」开始生成测试文件").clicked() {
                self.state.current_tab = AppTab::WriteTest;
            }
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &self.state.speed_history, "读取", "read_speed_plot");
        ui.add_space(10.0);
        render_heatmap(ui, &self.state.heatmap_blocks);

        // 测试报告导出面板
        if self.state.status == TaskStatus::Finished && self.state.stats.good + self.state.stats.bad > 0 {
            ui.add_space(10.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.heading("测试诊断报告就绪:");
                    if ui.button(RichText::new("一键复制完整测试诊断报告到剪贴板").color(Color32::GREEN)).clicked() {
                        let report = format!(
                            "# F3-Win 闪存检测诊断报告\n\
                            - 测试路径: {:?}\n\
                            - 平均读速: {:.2} MB/s\n\
                            - 健康扇区 (Good): {}\n\
                            - 损毁扇区 (Bad): {}\n\
                            - 变异扇区 (Changed): {}\n\
                            - 回绕覆写 (Overwritten): {}\n\
                            - 诊断结论: {}\n",
                            self.state.selected_path,
                            self.state.current_speed_mb,
                            self.state.stats.good,
                            self.state.stats.bad,
                            self.state.stats.changed,
                            self.state.stats.overwritten,
                            if self.state.stats.overwritten > 0 { "确认为虚标扩容假盘！" } else if self.state.stats.bad > 0 { "存在闪存物理坏道！" } else { "全部扇区健康合规！" }
                        );
                        ui.output_mut(|o| o.copied_text = report);
                    }
                });
            });
        }
    }

    fn render_probe_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("闪电裸盘跳跃探测 (f3probe)");
        ui.colored_label(Color32::YELLOW, "警告：本功能直接读写底层物理扇区，在 1~3 分钟内揭露虚标真面目，运行前会自动卸载卷！");
        ui.separator();

        // 智能联动：如果尚未手动选中设备，根据当前选中的卷路径自动定位底层物理磁盘
        if self.state.selected_device.is_none() {
            if let Some(first_char) = self.state.selected_path.to_str().and_then(|s| s.chars().next()) {
                if let Some(target_drive_num) = crate::core::devices::get_physical_drive_number_for_volume(first_char) {
                    if let Some(found_dev) = self.state.devices.iter().find(|d| d.drive_index == target_drive_num && d.is_safe_target) {
                        self.state.selected_device = Some(found_dev.clone());
                    }
                }
            }
        }

        let devices = self.state.devices.clone();
        let mut selected = self.state.selected_device.clone();
        let mut trigger_refresh = false;
        render_device_selector(ui, &devices, &mut selected, || {
            trigger_refresh = true;
        });
        self.state.selected_device = selected;
        if trigger_refresh {
            self.state.devices = PhysicalDeviceInfo::enumerate_devices();
            self.state.selected_device = self.state.devices.iter().find(|d| d.is_safe_target).cloned();
        }

        ui.add_space(15.0);

        if self.state.status != TaskStatus::Running {
            if ui.button(RichText::new("启动裸盘闪电探测").size(16.0).color(Color32::RED)).clicked() {
                self.start_probe_task();
            }
        } else {
            if ui.button(RichText::new("终止探测").size(16.0)).clicked() {
                self.state.cancel_flag.store(true, Ordering::Relaxed);
            }
        }

        ui.add_space(15.0);
        ui.add(ProgressBar::new(self.state.progress_pct).show_percentage());

        if let Some(ref res) = self.state.probe_result {
            ui.group(|ui| {
                ui.heading("探测诊断报告:");
                ui.label(format!("设备标称容量: {:.2} GB", res.announced_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)));
                ui.label(format!("物理真实容量: {:.2} GB", res.real_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)));

                if res.is_fake {
                    ui.colored_label(Color32::RED, "结论：确认为虚标扩容假盘！强烈建议前往「分区自愈」页面进行安全截断修复！");
                } else {
                    ui.colored_label(Color32::GREEN, "结论：物理容量完全合规，未检测到扇区回绕欺骗！");
                }
            });
        }
    }

    fn render_fix_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("扩容盘分区自愈修复 (f3fix)");
        ui.label("根据探测到的真实容量边界，重写安全分区表，让扩容假盘变回稳定耐用的小容量真盘。");
        ui.separator();

        if let Some(ref plan) = self.state.fix_plan {
            ui.group(|ui| {
                ui.label(format!("目标物理磁盘: PhysicalDrive{}", plan.disk_index));
                ui.label(format!("真实可用容量: {:.2} GB", plan.real_bytes as f64 / (1024.0 * 1024.0 * 1024.0)));
                ui.label(format!("建议截断分区大小: {} MB", plan.safe_partition_mb));
                ui.label(format!("推荐文件系统: {}", plan.file_system));

                ui.add_space(10.0);
                if ui.button(RichText::new("一键执行全自动安全修复").size(16.0).color(Color32::GREEN)).clicked() {
                    match plan.execute_auto_fix() {
                        Ok(msg) => self.state.status_message = format!("修复成功: {}", msg),
                        Err(e) => self.state.status_message = format!("修复失败: {}", e),
                    }
                }

                if ui.button("导出 diskpart 脚本到桌面").clicked() {
                    let desk = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Desktop\\f3_fix.diskpart");
                    let _ = plan.export_script_to_file(&desk);
                    self.state.status_message = format!("脚本已导出至 {:?}", desk);
                }
            });
        } else {
            ui.label("暂无待修复方案，请先在「闪电探测」标签页完成假盘探测。");
        }
    }

    fn render_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("高级防伪与性能参数设置");
        ui.separator();

        ui.checkbox(&mut self.state.use_random_salt, "启用随机加盐防伪模式 (防止恶劣固件针对固定序列作弊)");
        if !self.state.use_random_salt {
            ui.label("当前处于标准模式 (Salt = 0)，与 H2testw 和原版 F3 保持 100% 双向兼容。");
        } else {
            ui.colored_label(Color32::YELLOW, "注意：启用随机加盐后生成的测试文件只能由本程序或原版 F3 带相同盐值读取！");
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
            ui.label(RichText::new("F3 原版高级区间测试选项 (类似 --start-at / --end-at):").strong());
            ui.horizontal(|ui| {
                ui.label("起始文件编号:");
                ui.add(egui::DragValue::new(&mut self.state.start_file_num).range(0..=10000).prefix("从第 ").suffix(" 个文件开始写入 (断点续测)"));
            });
            ui.horizontal(|ui| {
                let mut limit_enabled = self.state.max_test_gb.is_some();
                if ui.checkbox(&mut limit_enabled, "限制最大测试容量 (GB):").changed() {
                    self.state.max_test_gb = if limit_enabled { Some(16) } else { None };
                }
                if let Some(ref mut gb) = self.state.max_test_gb {
                    ui.add(egui::DragValue::new(gb).range(1..=4096).suffix(" GB"));
                }
            });
        });
    }

    fn start_write_task(&mut self) {
        self.state.reset_telemetry();
        self.state.status = TaskStatus::Running;
        self.state.status_message = "正在执行无缓冲直通写入...".to_string();
        self.start_instant = Some(Instant::now());
        self.state.speed_history.push([0.0, 0.0]);

        let (tx, rx) = unbounded();
        self.state.write_rx = Some(rx);

        let max_b = self.state.max_test_gb.map(|gb| gb * 1024 * 1024 * 1024);
        let config = F3WriterConfig {
            target_dir: self.state.selected_path.clone(),
            start_file_num: self.state.start_file_num,
            max_bytes: max_b,
            chunk_size: self.state.chunk_size_mb * 1024 * 1024,
            salt: if self.state.use_random_salt { 0x5A5A_A5A5_1234_5678 } else { 0 },
        };

        let flag = self.state.cancel_flag.clone();
        std::thread::spawn(move || {
            let writer = F3Writer::new(config, flag);
            writer.run(tx);
        });
    }

    fn start_read_task(&mut self) {
        self.state.reset_telemetry();
        self.state.status = TaskStatus::Running;
        self.state.status_message = "正在执行流式校验...".to_string();
        self.start_instant = Some(Instant::now());

        let (tx, rx) = unbounded();
        self.state.read_rx = Some(rx);

        let config = F3ReaderConfig {
            target_dir: self.state.selected_path.clone(),
            salt: if self.state.use_random_salt { 0x5A5A_A5A5_1234_5678 } else { 0 },
            chunk_size: self.state.chunk_size_mb * 1024 * 1024,
        };

        let flag = self.state.cancel_flag.clone();
        std::thread::spawn(move || {
            let reader = F3Reader::new(config, flag);
            reader.run(tx);
        });
    }

    fn start_probe_task(&mut self) {
        let dev_opt = self.state.selected_device.clone();
        if let Some(dev) = dev_opt {
            // 探测前安全操作：自动尝试锁定并卸载该物理设备上挂载的全部卷，防止 Windows 写冲突
            for &vol_letter in &dev.mounted_volumes {
                let _ = PhysicalDeviceInfo::lock_and_dismount_volume(vol_letter);
            }

            self.state.reset_telemetry();
            self.state.status = TaskStatus::Running;
            self.state.status_message = "正在启动跳跃探针探测...".to_string();

            let (tx, rx) = unbounded();
            self.state.probe_rx = Some(rx);

            let config = F3ProbeConfig {
                physical_device_path: dev.path,
                total_sectors: dev.total_bytes / 512,
            };

            let flag = self.state.cancel_flag.clone();
            std::thread::spawn(move || {
                let probe = F3Probe::new(config, flag);
                probe.run(tx);
            });
        } else {
            self.state.status_message = "请先选择一个合规安全的移动设备！".to_string();
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
