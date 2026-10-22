use egui::{Color32, Rect, Response, RichText, Sense, Ui, Vec2};
use crate::core::algo::BlockState;
use crate::gui::theme;

pub fn render_heatmap(ui: &mut Ui, blocks: &[BlockState]) {
    let frame = egui::Frame::none()
        .fill(theme::SURFACE_BG)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_HAIRLINE))
        .rounding(egui::Rounding::ZERO)
        .inner_margin(egui::Margin::symmetric(14.0, 10.0));

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("扇区健康分布矩阵 (1000 采样区间)")
                    .size(13.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                legend_badge(ui, theme::ACCENT_PURPLE, "回绕覆盖");
                ui.add_space(4.0);
                legend_badge(ui, theme::ACCENT_AMBER, "数据异常");
                ui.add_space(4.0);
                legend_badge(ui, theme::ACCENT_RED, "扇区损坏");
                ui.add_space(4.0);
                legend_badge(ui, theme::ACCENT_GREEN, "校验正常");
                ui.add_space(4.0);
                legend_badge(ui, theme::STATUS_UNTESTED, "待测试");
            });
        });

        ui.add_space(8.0);

        let total_cells = blocks.len();
        let cols = 50;
        let rows = total_cells.div_ceil(cols);

        let available_w = ui.available_width();
        let gap = 2.0_f32;
        let cell_width = ((available_w - (cols as f32 - 1.0) * gap) / (cols as f32)).max(2.0);
        // 根据单元格宽度动态联动高度，保持晶圆微网格黄金比例 (约 4:3)，消除宽屏扁长失真
        let cell_height = (cell_width * 0.75).clamp(6.0, 11.0);
        let desired_size = Vec2::new(available_w, (rows as f32) * (cell_height + gap) + 4.0);
        let (rect, _response): (Rect, Response) = ui.allocate_exact_size(desired_size, Sense::hover());

        let painter = ui.painter_at(rect);

        for (i, &state) in blocks.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;

            let x = rect.min.x + col as f32 * (cell_width + gap);
            let y = rect.min.y + row as f32 * (cell_height + gap);

            let cell_rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_width, cell_height));

            let color = match state {
                BlockState::Untested => theme::STATUS_UNTESTED,
                BlockState::Good => theme::ACCENT_GREEN,
                BlockState::Bad => theme::ACCENT_RED,
                BlockState::Changed => theme::ACCENT_AMBER,
                BlockState::Overwritten => theme::ACCENT_PURPLE,
            };

            painter.rect_filled(cell_rect, egui::Rounding::ZERO, color);
        }
    });
}

fn legend_badge(ui: &mut Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(9.0), Sense::hover());
        ui.painter().rect_filled(rect, egui::Rounding::ZERO, color);
        ui.label(RichText::new(text).size(11.0).color(theme::TEXT_MUTED));
    });
}
