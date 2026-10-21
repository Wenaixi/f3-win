use egui::{Color32, RichText, Ui};
use egui_plot::{Line, Plot, PlotPoints};
use crate::gui::theme;

pub fn render_speed_plot(ui: &mut Ui, history: &[[f64; 2]], title: &str, plot_id: &str) {
    let frame = egui::Frame::none()
        .fill(theme::SURFACE_BG)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(14.0, 10.0));

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("传输速率波动曲线 · {}", title))
                    .size(13.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(last) = history.last() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("({:.1}s)", last[0])).size(11.0).color(theme::TEXT_MUTED));
                        ui.label(
                            RichText::new(format!("{:.2} MB/s", last[1]))
                                .size(13.0)
                                .strong()
                                .color(theme::ACCENT_CYAN),
                        );
                        ui.label(RichText::new("瞬时:").size(11.0).color(theme::TEXT_MUTED));
                    });
                } else {
                    ui.label(RichText::new("等待测试启动采样").size(11.0).color(theme::TEXT_MUTED));
                }
            });
        });

        ui.add_space(6.0);

        // 自适应计算视野极值，保证 SLC 掉速与物理带宽断崖 100% 居中跟焦
        let mut max_speed = 30.0f64;
        let mut max_time = 10.0f64;
        for &[t, s] in history {
            if s > max_speed { max_speed = s; }
            if t > max_time { max_time = t; }
        }

        let is_write = title.contains("写入");
        let line_color = if is_write {
            theme::ACCENT_CYAN
        } else {
            theme::ACCENT_GREEN
        };

        let points: PlotPoints = history.iter().map(|&[t, s]| [t, s]).collect();
        let line = Line::new(points)
            .color(line_color)
            .width(2.0_f32)
            .fill(0.0_f32)
            .name("物理带宽 (MB/s)");

        Plot::new(plot_id)
            .height(180.0)
            .show_axes([true, true])
            .show_grid(true)
            .allow_zoom(true)
            .allow_drag(true)
            .allow_scroll(false)
            .include_x(0.0)
            .include_x(max_time * 1.05)
            .include_y(0.0)
            .include_y(max_speed * 1.15)
            .show(ui, |plot_ui| {
                plot_ui.line(line);
            });
    });
}
