use egui::Ui;
use egui_plot::{Line, Plot, PlotPoints};

pub fn render_speed_plot(ui: &mut Ui, history: &[[f64; 2]], title: &str) {
    ui.group(|ui| {
        ui.label(format!("实时速度曲线 ({}) - 纵轴: MB/s，横轴: 时间(秒)", title));

        let points: PlotPoints = history.iter().map(|&[t, s]| [t, s]).collect();
        let line = Line::new(points)
            .color(egui::Color32::from_rgb(0, 190, 255))
            .width(2.0_f32)
            .name("实时速率 (MB/s)");

        Plot::new("speed_plot")
            .height(180.0)
            .show_axes([true, true])
            .show_grid(true)
            .allow_zoom(false)
            .allow_drag(false)
            .show(ui, |plot_ui| {
                plot_ui.line(line);
            });
    });
}
