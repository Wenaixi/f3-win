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
                    
                    if msg.error.is_none() && self.state.auto_cleanup_after_read {
                        if let Ok((cnt, bytes)) = crate::core::reader::cleanup_h2w_files(&self.state.selected_path) {
                            self.state.status_message = format!("校验完成，已自动清理 {} 个测试文件，释放 {:.2} GB 空间！", cnt, bytes as f64 / (1024.0 * 1024.0 * 1024.0));
                            self.state.volumes = VolumeInfo::enumerate_volumes();
                        } else {
                            self.state.status_message = "校验测试完成".to_string();
                        }
                    } else {
                        self.state.status_message = msg.error.unwrap_or_else(|| "校验测试完成".to_string());
                    }
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
                ui.heading(RichText::new("F3 闪存完整性测试").strong());
                ui.separator();

                ui.selectable_value(&mut self.state.current_tab, AppTab::WriteTest, "写入测试 (f3write)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::ReadVerify, "读取校验 (f3read)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::RapidProbe, "容量探测 (f3probe)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::PartitionFix, "分区修复 (f3fix)");
                ui.selectable_value(&mut self.state.current_tab, AppTab::Settings, "高级设置");
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
                    ui.label("Direct I/O 直写模式");
                });
            });
        });
    }
}

impl F3App {
    fn render_write_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("写入测试 (f3write)");
        ui.label("向目标磁盘写入 1GB 大小的 .h2w 文件以测试写速并准备校验。");
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
            if ui.button(RichText::new("开始写入").size(15.0)).clicked() {
                self.start_write_task();
            }
        } else {
            if ui.button(RichText::new("停止写入").size(15.0)).clicked() {
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
                ui.label(RichText::new("当前写入速度").weak().size(12.0));
                ui.label(RichText::new(format!("{:.2} MB/s", self.state.current_speed_mb)).strong().size(20.0).color(Color32::from_rgb(0, 200, 255)));
            });

            // 卡片 2: 已写入数据
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("已写入数据").weak().size(12.0));
                ui.label(RichText::new(format!("{:.2} GB", self.state.total_bytes_processed as f64 / (1024.0 * 1024.0 * 1024.0))).strong().size(20.0));
            });

            // 卡片 3: 动态 ETA 倒计时
            ui.group(|ui| {
                ui.set_width(card_width);
                ui.label(RichText::new("预计剩余时间").weak().size(12.0));
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
                ui.label(RichText::new("I/O 模式").weak().size(12.0));
                ui.label(RichText::new("Direct I/O (无缓存)").strong().size(18.0));
            });
        });

        // 便捷按钮区
        ui.horizontal(|ui| {
            if ui.button("在资源管理器中打开").clicked() {
                let _ = std::process::Command::new("explorer").arg(&self.state.selected_path).spawn();
            }
            if ui.button(RichText::new("清理测试文件 (.h2w)")).clicked() {
                match crate::core::reader::cleanup_h2w_files(&self.state.selected_path) {
                    Ok((cnt, bytes)) => {
                        self.state.status_message = format!("已成功清理 {} 个文件，释放 {:.2} GB 磁盘空间！", cnt, bytes as f64 / (1024.0 * 1024.0 * 1024.0));
                        self.state.volumes = VolumeInfo::enumerate_volumes();
                    }
                    Err(e) => self.state.status_message = format!("清理失败: {}", e),
                }
            }
        });

        ui.add_space(10.0);
        render_speed_plot(ui, &self.state.speed_history, "写入", "write_speed_plot");
    }

    fn render_read_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("读取校验 (f3read)");
        ui.label("读取磁盘上的 .h2w 文件比对数据，检查损坏扇区与虚标覆盖。");
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
            if ui.button(RichText::new("开始校验").size(15.0)).clicked() {
                self.start_read_task();
            }
        } else {
            if ui.button(RichText::new("停止校验").size(15.0)).clicked() {
                self.state.cancel_flag.store(true, Ordering::Relaxed);
            }
        }

        ui.add_space(10.0);

        // 校验统计计数
        ui.horizontal(|ui| {
            ui.colored_label(Color32::from_rgb(46, 204, 113), format!("正常: {}", self.state.stats.good));
            ui.separator();
            ui.colored_label(Color32::from_rgb(231, 76, 60), format!("损坏: {}", self.state.stats.bad));
            ui.separator();
            ui.colored_label(Color32::from_rgb(241, 196, 15), format!("异常: {}", self.state.stats.changed));
            ui.separator();
            ui.colored_label(Color32::from_rgb(155, 89, 182), format!("覆盖: {}", self.state.stats.overwritten));
        });

        // 便捷按钮与自动清理控制
        ui.horizontal(|ui| {
            if ui.button("在文件资源管理器中打开此路径").clicked() {
                let _ = std::process::Command::new("explorer").arg(&self.state.selected_path).spawn();
            }
            if ui.button("转到写入页面").clicked() {
                self.state.current_tab = AppTab::WriteTest;
            }
            if ui.button(RichText::new("一键清理所有 .h2w 测试文件 (释放空间)").color(Color32::from_rgb(255, 140, 0))).clicked() {
                match crate::core::reader::cleanup_h2w_files(&self.state.selected_path) {
                    Ok((cnt, bytes)) => {
                        self.state.status_message = format!("已成功清理 {} 个文件，释放 {:.2} GB 磁盘空间！", cnt, bytes as f64 / (1024.0 * 1024.0 * 1024.0));
                        self.state.volumes = VolumeInfo::enumerate_volumes();
                    }
                    Err(e) => self.state.status_message = format!("清理失败: {}", e),
                }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.state.auto_cleanup_after_read, "校验完成后自动删除测试文件");
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
        ui.heading("容量探测 (f3probe)");
        ui.label("直接向物理磁盘写入探针测试实际容量。测试前会自动卸载卷，请备份重要数据。");
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
            if ui.button(RichText::new("开始探测").size(15.0)).clicked() {
                self.start_probe_task();
            }
        } else {
            if ui.button(RichText::new("停止探测").size(15.0)).clicked() {
                self.state.cancel_flag.store(true, Ordering::Relaxed);
            }
        }

        ui.add_space(15.0);
        ui.add(ProgressBar::new(self.state.progress_pct).show_percentage());

        if let Some(ref res) = self.state.probe_result {
            if let Some(ref err) = res.error {
                ui.group(|ui| {
                    ui.colored_label(Color32::RED, RichText::new(format!("无法执行探测: {}", err)).strong().size(15.0));
                    ui.label("提示: 请确认已允许管理员权限，且设备未被其他程序占用。");
                });
            } else {
                ui.group(|ui| {
                    ui.heading("探测结果:");
                    ui.label(format!("设备标称容量: {:.2} GB", res.announced_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)));
                    ui.label(format!("物理真实容量: {:.2} GB", res.real_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)));

                    if res.is_fake {
                        ui.colored_label(Color32::RED, "检测到容量虚标。建议前往「分区修复」调整可用分区大小。");
                    } else {
                        ui.colored_label(Color32::GREEN, "容量正常，未发现地址回绕。");
                    }
                });
            }
        }
    }

    fn render_fix_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("分区修复 (f3fix)");
        ui.label("根据探测出的实际物理容量调整分区大小，避免使用超出物理范围的扇区。");
        ui.separator();

        if let Some(ref plan) = self.state.fix_plan {
            ui.group(|ui| {
                ui.label(format!("目标物理磁盘: PhysicalDrive{}", plan.disk_index));
                ui.label(format!("真实可用容量: {:.2} GB", plan.real_bytes as f64 / (1024.0 * 1024.0 * 1024.0)));
                ui.label(format!("建议截断分区大小: {} MB", plan.safe_partition_mb));
                ui.label(format!("推荐文件系统: {}", plan.file_system));

                ui.add_space(10.0);
                if ui.button(RichText::new("应用安全分区 (运行 diskpart)").size(15.0)).clicked() {
                    match plan.execute_auto_fix() {
                        Ok(msg) => self.state.status_message = format!("修复成功: {}", msg),
                        Err(e) => self.state.status_message = format!("修复失败: {}", e),
                    }
                }

                if ui.button("导出 diskpart 脚本").clicked() {
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
                ui.add(egui::DragValue::new(&mut self.state.start_file_num).range(0..=10000).prefix("从第 ").suffix(" 个文件开始写入"));
            });
            ui.horizontal(|ui| {
                let mut end_enabled = self.state.end_file_num.is_some();
                if ui.checkbox(&mut end_enabled, "指定结束文件编号:").changed() {
                    self.state.end_file_num = if end_enabled { Some(self.state.start_file_num + 5) } else { None };
                }
                if let Some(ref mut end) = self.state.end_file_num {
                    ui.add(egui::DragValue::new(end).range(0..=10000).prefix("写到第 ").suffix(" 个文件截止"));
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
            end_file_num: self.state.end_file_num,
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
