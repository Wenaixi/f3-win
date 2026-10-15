use egui::Ui;
use crate::core::devices::PhysicalDeviceInfo;

pub fn render_device_selector(
    ui: &mut Ui,
    devices: &[PhysicalDeviceInfo],
    selected: &mut Option<PhysicalDeviceInfo>,
    on_refresh: impl FnOnce(),
) {
    ui.horizontal(|ui| {
        ui.label("目标物理磁盘:");

        let current_text = match selected {
            Some(dev) => {
                let vol_tag = if !dev.mounted_volumes.is_empty() {
                    let letters: Vec<String> = dev.mounted_volumes.iter().map(|c| format!("{}:\\", c)).collect();
                    format!(" [对应盘符: {}]", letters.join(", "))
                } else {
                    String::new()
                };
                format!(
                    "[{}] PhysicalDrive{} - {} {} ({:.1} GB){}",
                    dev.bus_type.display_name(),
                    dev.drive_index,
                    dev.vendor_id,
                    dev.product_id,
                    dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                    vol_tag,
                )
            }
            None => "请选择存储设备...".to_string(),
        };

        egui::ComboBox::from_id_salt("device_combo")
            .selected_text(current_text)
            .width(460.0)
            .show_ui(ui, |ui| {
                for dev in devices {
                    let is_safe = dev.is_safe_target;
                    let vol_tag = if !dev.mounted_volumes.is_empty() {
                        let letters: Vec<String> = dev.mounted_volumes.iter().map(|c| format!("{}:\\", c)).collect();
                        format!(" [对应盘符: {}]", letters.join(", "))
                    } else {
                        String::new()
                    };

                    let label_text = if is_safe {
                        format!(
                            "[{}] PhysicalDrive{} - {} {} ({:.1} GB){}",
                            dev.bus_type.display_name(),
                            dev.drive_index,
                            dev.vendor_id,
                            dev.product_id,
                            dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                            vol_tag,
                        )
                    } else {
                        format!(
                            "[锁定保护] PhysicalDrive{} - {} (系统或内部磁盘){}",
                            dev.drive_index,
                            dev.product_id,
                            vol_tag,
                        )
                    };

                    let resp = ui.selectable_label(
                        selected.as_ref().map(|d| d.drive_index) == Some(dev.drive_index),
                        label_text,
                    );

                    if resp.clicked() && is_safe {
                        *selected = Some(dev.clone());
                    }
                }
            });

        if ui.button("刷新").clicked() {
            on_refresh();
        }
    });
}

use crate::core::devices::VolumeInfo;
use std::path::PathBuf;

pub fn render_volume_selector(
    ui: &mut Ui,
    id_salt: &str,
    volumes: &[VolumeInfo],
    selected_idx: &mut usize,
    selected_path: &mut PathBuf,
    on_refresh: impl FnOnce(),
) {
    ui.horizontal(|ui| {
        ui.label("选择磁盘:");
        let current_label = if *selected_idx < volumes.len() {
            volumes[*selected_idx].display_name()
        } else {
            "自定义路径".to_string()
        };

        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(current_label)
            .width(420.0)
            .show_ui(ui, |ui| {
                for (idx, vol) in volumes.iter().enumerate() {
                    let is_selected = *selected_idx == idx;
                    if ui.selectable_label(is_selected, vol.display_name()).clicked() {
                        *selected_idx = idx;
                        *selected_path = vol.root_path.clone();
                    }
                }
            });

        if ui.button("刷新").clicked() {
            on_refresh();
        }
    });

    ui.horizontal(|ui| {
        ui.label("测试路径:");
        let mut path_str = selected_path.to_string_lossy().to_string();
        if ui.text_edit_singleline(&mut path_str).changed() {
            *selected_path = PathBuf::from(path_str);
        }
    });
}
