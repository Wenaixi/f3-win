use eframe::egui::{
    self, Color32, Margin, Response, Rounding, Stroke, Ui, Vec2, WidgetText,
};

// ==========================================
// 设计系统色彩 Tokens (Color Tokens)
// ==========================================

// 1. 画布与表面色阶 (Surface Hierarchy)
pub const CANVAS_BG: Color32 = Color32::from_rgb(12, 14, 18);
pub const PANEL_BG: Color32 = Color32::from_rgb(17, 20, 26);
pub const SURFACE_BG: Color32 = Color32::from_rgb(23, 27, 35);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(31, 37, 48);
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(41, 48, 62);
pub const SURFACE_FAINT: Color32 = Color32::from_rgb(18, 22, 28);
pub const POPUP_BG: Color32 = Color32::from_rgb(20, 24, 32);

// 2. 极细微边框体系 (Border Hierarchy)
pub const BORDER_HAIRLINE: Color32 = Color32::from_rgb(36, 42, 54);
pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(46, 54, 70);
pub const BORDER_HOVER: Color32 = Color32::from_rgb(76, 88, 112);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(115, 130, 155);
pub const BORDER_WHITE: Color32 = Color32::from_rgb(220, 228, 240);

// 3. 文字层级 (Typography Tokens)
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(242, 245, 248);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(155, 165, 178);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(98, 108, 124);
pub const TEXT_INVERTED: Color32 = Color32::from_rgb(12, 14, 18);

// 4. 严格克制的语义指示色 (Semantic Tokens)
pub const ACCENT_WHITE: Color32 = Color32::from_rgb(248, 249, 250);
pub const STATUS_GOOD: Color32 = Color32::from_rgb(16, 185, 129);   // 冷翡翠绿
pub const STATUS_BAD: Color32 = Color32::from_rgb(239, 68, 68);     // 警示绯红
pub const STATUS_WARN: Color32 = Color32::from_rgb(245, 158, 11);   // 琥珀金黄
pub const STATUS_CYAN: Color32 = Color32::from_rgb(56, 189, 248);   // 微弱冰蓝数据流

// ==========================================
// 主题配置注入 (Theme Installation)
// ==========================================

/// 将现代黑白高级设计系统注入到 egui 上下文中
pub fn install_modern_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    // 画布与面板全局配色
    visuals.panel_fill = PANEL_BG;
    visuals.window_fill = POPUP_BG;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER_HAIRLINE);
    visuals.window_rounding = Rounding::same(8.0);
    visuals.faint_bg_color = SURFACE_FAINT;
    visuals.extreme_bg_color = CANVAS_BG;
    visuals.hyperlink_color = ACCENT_WHITE;
    visuals.warn_fg_color = STATUS_WARN;
    visuals.error_fg_color = STATUS_BAD;

    // 控件交互各状态视觉规范
    visuals.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: SURFACE_BG,
        weak_bg_fill: Color32::TRANSPARENT,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HAIRLINE),
        rounding: Rounding::same(5.0),
        fg_stroke: Stroke::new(1.0_f32, TEXT_SECONDARY),
        expansion: 0.0,
    };

    visuals.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: SURFACE_BG,
        weak_bg_fill: SURFACE_BG,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HAIRLINE),
        rounding: Rounding::same(5.0),
        fg_stroke: Stroke::new(1.0_f32, TEXT_PRIMARY),
        expansion: 0.0,
    };

    visuals.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: SURFACE_HOVER,
        weak_bg_fill: SURFACE_HOVER,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HOVER),
        rounding: Rounding::same(5.0),
        fg_stroke: Stroke::new(1.0_f32, ACCENT_WHITE),
        expansion: 0.0,
    };

    visuals.widgets.active = egui::style::WidgetVisuals {
        bg_fill: SURFACE_ACTIVE,
        weak_bg_fill: SURFACE_ACTIVE,
        bg_stroke: Stroke::new(1.0_f32, BORDER_STRONG),
        rounding: Rounding::same(5.0),
        fg_stroke: Stroke::new(1.0_f32, ACCENT_WHITE),
        expansion: 0.0,
    };

    visuals.widgets.open = egui::style::WidgetVisuals {
        bg_fill: SURFACE_ACTIVE,
        weak_bg_fill: SURFACE_ACTIVE,
        bg_stroke: Stroke::new(1.0_f32, BORDER_STRONG),
        rounding: Rounding::same(5.0),
        fg_stroke: Stroke::new(1.0_f32, ACCENT_WHITE),
        expansion: 0.0,
    };

    // 选中与高亮规范
    visuals.selection.bg_fill = Color32::from_rgb(40, 48, 64);
    visuals.selection.stroke = Stroke::new(1.0_f32, BORDER_STRONG);

    ctx.set_visuals(visuals);

    // 全局排版与间距配置
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.window_margin = Margin::same(12.0);
        style.spacing.combo_width = 320.0;
    });
}

// ==========================================
// 统一高级组件渲染原语 (Modern Components)
// ==========================================

/// 渲染现代 Bento-Grid 高级卡片容器
pub fn bento_card<R>(
    ui: &mut Ui,
    width: Option<f32>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let frame = egui::Frame::none()
        .fill(SURFACE_BG)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(12.0));

    if let Some(w) = width {
        ui.allocate_ui(egui::vec2(w, 0.0), |ui| {
            ui.set_width(w);
            frame.show(ui, add_contents).inner
        })
    } else {
        frame.show(ui, add_contents)
    }
}

/// 渲染高对比度主操作反白按钮 (纯白背景 / 纯黑文字)
pub fn primary_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(ACCENT_WHITE)
        .stroke(Stroke::new(1.0_f32, BORDER_WHITE))
        .rounding(Rounding::same(5.0))
}

/// 渲染危险/中止操作按钮 (暗底 / 绯红微弱边框)
pub fn danger_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(Color32::from_rgb(45, 20, 24))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(180, 50, 60)))
        .rounding(Rounding::same(5.0))
}

/// 渲染次级幽灵透明按钮 (透明底 / 极细边框)
pub fn ghost_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .rounding(Rounding::same(5.0))
}

/// 渲染微型状态指示圆点 (Status Dot)
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    
    // 微弱柔和外圈光晕
    let halo_color = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40);
    painter.circle_filled(center, 4.5, halo_color);
    
    // 核心实心小圆点
    painter.circle_filled(center, 3.0, color);
}

/// 渲染极简全圆角胶囊标签 (Pill Badge)
pub fn pill_badge(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) {
    let padding = egui::vec2(8.0, 3.0);
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(11.0),
        fg,
    );
    let desired_size = galley.size() + padding * 2.0;
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(12.0), bg);
    painter.galley(rect.min + padding, galley, fg);
}
