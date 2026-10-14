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
            Some(dev) => format!(
                "[{}] PhysicalDrive{} - {} {} ({:.1} GB)",
                dev.bus_type.display_name(),
                dev.drive_index,
                dev.vendor_id,
                dev.product_id,
                dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            None => "请选择存储设备...".to_string(),
        };

        egui::ComboBox::from_id_salt("device_combo")
            .selected_text(current_text)
            .width(450.0)
            .show_ui(ui, |ui| {
                for dev in devices {
                    let is_safe = dev.is_safe_target;
                    let label_text = if is_safe {
                        format!(
                            "[{}] PhysicalDrive{} - {} {} ({:.1} GB)",
                            dev.bus_type.display_name(),
                            dev.drive_index,
                            dev.vendor_id,
                            dev.product_id,
                            dev.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        )
                    } else {
                        format!(
                            "[锁定保护] PhysicalDrive{} - {} (系统或内部磁盘)",
                            dev.drive_index,
                            dev.product_id
                        )
                    };

                    let resp = ui.selectable_label(
                        selected.as_ref().map(|d| d.drive_index) == Some(dev.drive_index),
                        label_text,
                    );

                    if resp.clicked() {
                        if is_safe {
                            *selected = Some(dev.clone());
                        }
                    }
                }
            });

        if ui.button("刷新设备列表").clicked() {
            on_refresh();
        }
    });
}
