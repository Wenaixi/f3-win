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

                    let mut resp = ui.add_enabled(
                        is_safe,
                        egui::SelectableLabel::new(
                            selected_idx == Some(idx),
                            label_text,
                        ),
                    );
                    if !is_safe {
                        resp = resp.on_disabled_hover_text("此设备为系统盘或内置固定总线磁盘，为保护系统与数据安全已硬锁定，严禁作为破坏性测试目标");
                    }

                    if resp.clicked() && is_safe {
                        inventory.select_device(idx);
                    }
                }
            });

        ui.add_space(4.0);
        if ui.add_sized([86.0, 26.0], theme::ghost_button("刷新设备 (F5)")).clicked() {
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
                    let label_text = if is_safe {
                        vol.display_name()
                    } else {
                        format!("{} [受保护系统卷]", vol.display_name())
                    };
                    let mut resp = ui.add_enabled(
                        is_safe,
                        egui::SelectableLabel::new(
                            selected_idx == idx,
                            label_text,
                        ),
                    );
                    if !is_safe {
                        resp = resp.on_disabled_hover_text("此分区位于系统引导盘或受保护磁盘上，为防止误操作导致系统崩溃已禁用");
                    }
                    if resp.clicked() && is_safe {
                        inventory.select_volume(idx);
                    }
                }
            });

        ui.add_space(4.0);
        if ui.add_sized([76.0, 26.0], theme::ghost_button("刷新卷 (F5)")).clicked() {
            inventory.refresh();
        }
    });
}
