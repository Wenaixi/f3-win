use egui::{Color32, Rect, Response, Sense, Ui, Vec2};
use crate::core::algo::BlockState;

pub fn render_heatmap(ui: &mut Ui, blocks: &[BlockState]) {
    ui.group(|ui| {
        ui.label("扇区健康分布图:");

        // 图例
        ui.horizontal(|ui| {
            legend_badge(ui, Color32::from_rgb(45, 52, 60), "未测试");
            legend_badge(ui, Color32::from_rgb(46, 204, 113), "正常");
            legend_badge(ui, Color32::from_rgb(231, 76, 60), "损坏");
            legend_badge(ui, Color32::from_rgb(241, 196, 15), "异常");
            legend_badge(ui, Color32::from_rgb(155, 89, 182), "覆盖");
        });

        let total_cells = blocks.len();
        let cols = 50;
        let rows = total_cells.div_ceil(cols);

        let desired_size = Vec2::new(ui.available_width(), (rows as f32) * 10.0 + 10.0);
        let (rect, _response): (Rect, Response) = ui.allocate_exact_size(desired_size, Sense::hover());

        let painter = ui.painter_at(rect);
        let cell_width = ((rect.width() - (cols as f32 - 1.0) * 2.0) / (cols as f32)).max(2.0);
        let cell_height = 8.0f32;

        for (i, &state) in blocks.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;

            let x = rect.min.x + col as f32 * (cell_width + 2.0);
            let y = rect.min.y + row as f32 * (cell_height + 2.0);

            let cell_rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_width, cell_height));

            let color = match state {
                BlockState::Untested => Color32::from_rgb(45, 52, 60),
                BlockState::Good => Color32::from_rgb(46, 204, 113),
                BlockState::Bad => Color32::from_rgb(231, 76, 60),
                BlockState::Changed => Color32::from_rgb(241, 196, 15),
                BlockState::Overwritten => Color32::from_rgb(155, 89, 182),
            };

            painter.rect_filled(cell_rect, 1.0, color);
        }
    });
}

fn legend_badge(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.label(text);
    });
}
