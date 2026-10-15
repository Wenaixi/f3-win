use egui::{Color32, RichText, Ui};
use egui_plot::{Line, Plot, PlotPoints};

pub fn render_speed_plot(ui: &mut Ui, history: &[[f64; 2]], title: &str, plot_id: &str) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("传输速率曲线 ({})", title)).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(last) = history.last() {
                    ui.label(RichText::new(format!("当前速率: {:.2} MB/s | 时间: {:.1} 秒", last[1], last[0])).color(Color32::from_rgb(0, 180, 220)));
                } else {
                    ui.label(RichText::new("等待测试开始").weak());
                }
            });
        });

        ui.add_space(4.0);

        // 计算视野范围
        let mut max_speed = 30.0f64;
        let mut max_time = 10.0f64;
        for &[t, s] in history {
            if s > max_speed { max_speed = s; }
            if t > max_time { max_time = t; }
        }

        let points: PlotPoints = history.iter().map(|&[t, s]| [t, s]).collect();
        let line = Line::new(points)
            .color(Color32::from_rgb(0, 160, 230))
            .width(2.0_f32)
            .name("速率 (MB/s)");

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
