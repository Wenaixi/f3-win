use egui::{RichText, Ui};
use crate::core::devices::StorageInventory;
use crate::gui::theme;

// 渲染存储设备选择器 (直接绑定 StorageInventory 深层模块)
pub fn render_storage_device_selector(
    ui: &mut Ui,
    inventory: &mut StorageInventory,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("目标物理磁盘:").size(12.5).color(theme::TEXT_SECONDARY));

        let current_text = match inventory.selected_device() {
            Some(dev) => {
                format!(
                    "[{}] PhysicalDrive{} - {} {} ({:.1} GB)",
                    dev.bus_type.display_name(),
                    dev.drive_index,
                    dev.vendor_id,
                    dev.product_id,
                    dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                )
            }
            None => "请选择存储设备...".to_string(),
        };

        let selected_idx = inventory.selected_device_index();

        egui::ComboBox::from_id_salt("storage_device_combo")
            .selected_text(RichText::new(current_text).size(12.5).color(theme::TEXT_PRIMARY))
            .width(480.0)
            .show_ui(ui, |ui| {
                let devices = inventory.devices().to_vec();
                if devices.is_empty() {
                    ui.label(RichText::new("未检测到物理磁盘设备").color(theme::TEXT_MUTED));
                }
                for (idx, dev) in devices.iter().enumerate() {
                    let is_safe = dev.is_safe_target;
                    let is_selected = selected_idx == Some(idx);
                    if is_safe {
                        let label_text = format!(
                            "[{}] PhysicalDrive{} - {} {} ({:.1} GB)",
                            dev.bus_type.display_name(),
                            dev.drive_index,
                            dev.vendor_id,
                            dev.product_id,
                            dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        );
                        let rich = RichText::new(label_text).size(12.0).color(theme::TEXT_PRIMARY);
                        if ui.selectable_label(is_selected, rich).clicked() {
                            inventory.select_device(idx);
                        }
                    } else {
                        let label_text = format!(
                            "[锁定保护] PhysicalDrive{} - {} (系统或内部磁盘)",
                            dev.drive_index,
                            dev.product_id,
                        );
                        let rich = RichText::new(label_text).size(12.0).color(theme::TEXT_MUTED);
                        let resp = ui.selectable_label(false, rich);
                        resp.on_hover_text("此设备为系统主盘或内部固定磁盘，为保护系统已硬隔离锁定，严禁作为破坏性测试目标");
                    }
                }
            });

        ui.add_space(8.0);
        let dev_btn = ui.add_sized([92.0, 28.0], theme::ghost_button("刷新设备 (F5)"));
        if dev_btn.on_hover_text("扫描并重新加载当前系统接入的物理磁盘设备 (快捷键 F5)").clicked() {
            inventory.refresh();
        }
    });
}

// 渲染逻辑卷选择器 (直接绑定 StorageInventory 深层模块)
pub fn render_storage_volume_selector(
    ui: &mut Ui,
    id_salt: &str,
    inventory: &mut StorageInventory,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("目标分区卷:").size(12.5).color(theme::TEXT_SECONDARY));
        let current_label = if let Some(vol) = inventory.selected_volume() {
            vol.display_name()
        } else {
            "未检测到可用分区".to_string()
        };

        let selected_idx = inventory.selected_volume_index();

        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(RichText::new(current_label).size(12.5).color(theme::TEXT_PRIMARY))
            .width(480.0)
            .show_ui(ui, |ui| {
                let volumes = inventory.volumes().to_vec();
                if volumes.is_empty() {
                    ui.label(RichText::new("未检测到存储卷，请插入 U 盘后点击刷新 (F5)").color(theme::TEXT_MUTED));
                }
                for (idx, vol) in volumes.iter().enumerate() {
                    let is_safe = vol.is_safe_target;
                    let is_selected = selected_idx == idx;
                    if is_safe {
                        let rich = RichText::new(vol.display_name()).size(12.0).color(theme::TEXT_PRIMARY);
                        if ui.selectable_label(is_selected, rich).clicked() {
                            inventory.select_volume(idx);
                        }
                    } else {
                        let label_text = format!("{} [锁定保护]", vol.display_name());
                        let rich = RichText::new(label_text).size(12.0).color(theme::TEXT_MUTED);
                        let resp = ui.selectable_label(false, rich);
                        resp.on_hover_text("此分区位于系统引导盘或受保护磁盘上，为防止误操作已硬隔离锁定");
                    }
                }
            });

        ui.add_space(8.0);
        let vol_btn = ui.add_sized([92.0, 28.0], theme::ghost_button("刷新卷 (F5)"));
        if vol_btn.on_hover_text("扫描并重新加载当前系统挂载的所有存储分区卷 (快捷键 F5)").clicked() {
            inventory.refresh();
        }
    });
}
