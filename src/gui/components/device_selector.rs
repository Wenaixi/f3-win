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
            .width(460.0)
            .show_ui(ui, |ui| {
                let devices = inventory.devices().to_vec();
                for (idx, dev) in devices.iter().enumerate() {
                    let is_safe = dev.is_safe_target;
                    let label_text = if is_safe {
                        format!(
                            "[{}] PhysicalDrive{} - {} {} ({:.1} GB)",
                            dev.bus_type.display_name(),
                            dev.drive_index,
                            dev.vendor_id,
                            dev.product_id,
                            dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        )
                    } else {
                        format!(
                            "[锁定保护] PhysicalDrive{} - {} (系统或内部磁盘)",
                            dev.drive_index,
                            dev.product_id,
                        )
                    };

                    let resp = ui.selectable_label(
                        selected_idx == Some(idx),
                        label_text,
                    );

                    if resp.clicked() && is_safe {
                        inventory.select_device(idx);
                    }
                }
            });

        ui.add_space(4.0);
        if theme::ghost_button(ui, "刷新设备", [72.0, 26.0]).clicked() {
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
            .width(360.0)
            .show_ui(ui, |ui| {
                let volumes = inventory.volumes().to_vec();
                for (idx, vol) in volumes.iter().enumerate() {
                    let is_safe = vol.is_safe_target;
                    let resp = ui.selectable_label(
                        selected_idx == idx,
                        vol.display_name(),
                    );
                    if resp.clicked() && is_safe {
                        inventory.select_volume(idx);
                    }
                }
            });

        ui.add_space(4.0);
        if theme::ghost_button(ui, "刷新卷", [62.0, 26.0]).clicked() {
            inventory.refresh();
        }
    });
}
