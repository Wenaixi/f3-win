use eframe::egui::{
    self, Color32, Margin, Rounding, Stroke, Ui, Vec2, WidgetText,
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
pub const BORDER_FOCUS: Color32 = Color32::from_rgb(180, 195, 215); // 冷银白焦点环，WCAG 2.1 AA 7.2:1 超高对比度

// 3. 文字层级 (Typography Tokens)
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(242, 245, 248);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(155, 165, 178);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(122, 134, 152); // 全面达标 WCAG AA 4.68:1 严苛对比度
pub const TEXT_INVERTED: Color32 = Color32::from_rgb(12, 14, 18);

// 4. 严格克制的语义指示色 (Semantic Tokens)
pub const ACCENT_WHITE: Color32 = Color32::from_rgb(248, 249, 250);
pub const ACCENT_GREEN: Color32 = Color32::from_rgb(16, 185, 129);   // 冷翡翠绿
pub const ACCENT_RED: Color32 = Color32::from_rgb(239, 68, 68);       // 警示绯红
pub const ACCENT_AMBER: Color32 = Color32::from_rgb(245, 158, 11);   // 琥珀金黄
pub const ACCENT_CYAN: Color32 = Color32::from_rgb(56, 189, 248);     // 微弱冰蓝数据流
pub const ACCENT_PURPLE: Color32 = Color32::from_rgb(179, 102, 255); // 洋紫覆写假盘色 (高对比增强)

// 语义微暗底色体系 (Badge Low-contrast Backgrounds)
pub const ACCENT_GREEN_BG: Color32 = Color32::from_rgb(16, 40, 28);
pub const ACCENT_RED_BG: Color32 = Color32::from_rgb(45, 16, 16);
pub const ACCENT_AMBER_BG: Color32 = Color32::from_rgb(45, 35, 16);
pub const ACCENT_CYAN_BG: Color32 = Color32::from_rgb(16, 32, 45);
pub const ACCENT_PURPLE_BG: Color32 = Color32::from_rgb(30, 14, 44);

pub const STATUS_GOOD: Color32 = ACCENT_GREEN;
pub const STATUS_BAD: Color32 = ACCENT_RED;
pub const STATUS_WARN: Color32 = ACCENT_AMBER;
pub const STATUS_CYAN: Color32 = ACCENT_CYAN;
pub const STATUS_PURPLE: Color32 = ACCENT_PURPLE;
pub const STATUS_UNTESTED: Color32 = Color32::from_rgb(24, 29, 38);

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
    visuals.window_rounding = Rounding::ZERO;
    visuals.menu_rounding = Rounding::ZERO;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.faint_bg_color = SURFACE_FAINT;
    visuals.extreme_bg_color = CANVAS_BG;
    visuals.hyperlink_color = ACCENT_WHITE;
    visuals.warn_fg_color = STATUS_WARN;
    visuals.error_fg_color = STATUS_BAD;

    // 控件交互各状态视觉规范：硬核直角工业风 (Rounding::ZERO)
    visuals.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: SURFACE_BG,
        weak_bg_fill: Color32::TRANSPARENT,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HAIRLINE),
        rounding: Rounding::ZERO,
        fg_stroke: Stroke::new(1.0_f32, TEXT_SECONDARY),
        expansion: 0.0,
    };

    visuals.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: SURFACE_BG,
        weak_bg_fill: SURFACE_BG,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HAIRLINE),
        rounding: Rounding::ZERO,
        fg_stroke: Stroke::new(1.0_f32, TEXT_PRIMARY),
        expansion: 0.0,
    };

    visuals.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: SURFACE_HOVER,
        weak_bg_fill: SURFACE_HOVER,
        bg_stroke: Stroke::new(1.0_f32, BORDER_HOVER),
        rounding: Rounding::ZERO,
        fg_stroke: Stroke::new(1.0_f32, ACCENT_WHITE),
        expansion: 0.0,
    };

    visuals.widgets.active = egui::style::WidgetVisuals {
        bg_fill: SURFACE_ACTIVE,
        weak_bg_fill: SURFACE_ACTIVE,
        bg_stroke: Stroke::new(1.0_f32, BORDER_STRONG),
        rounding: Rounding::ZERO,
        fg_stroke: Stroke::new(1.0_f32, ACCENT_WHITE),
        expansion: 0.0,
    };

    visuals.widgets.open = egui::style::WidgetVisuals {
        bg_fill: SURFACE_ACTIVE,
        weak_bg_fill: SURFACE_ACTIVE,
        bg_stroke: Stroke::new(1.0_f32, BORDER_STRONG),
        rounding: Rounding::ZERO,
        fg_stroke: Stroke::new(1.0_f32, ACCENT_WHITE),
        expansion: 0.0,
    };

    // 选中与高亮规范：纯白清晰勾选指示
    visuals.selection.bg_fill = Color32::from_rgb(40, 48, 64);
    visuals.selection.stroke = Stroke::new(1.5_f32, ACCENT_WHITE);

    ctx.set_visuals(visuals);

    // 全局排版、间距与键盘无障碍交互配置 (WCAG 2.1 AA)
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

/// 辅助转换 trait，使 bento_card 支持 f32 与 Option<f32> 零成本自适应挂载
pub trait IntoOptionF32 {
    fn into_option_f32(self) -> Option<f32>;
}

impl IntoOptionF32 for f32 {
    #[inline]
    fn into_option_f32(self) -> Option<f32> {
        Some(self)
    }
}

impl IntoOptionF32 for Option<f32> {
    #[inline]
    fn into_option_f32(self) -> Option<f32> {
        self
    }
}

/// 渲染现代 Bento-Grid 高级直角卡片容器
pub fn bento_card<R>(
    ui: &mut Ui,
    width: impl IntoOptionF32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let frame = egui::Frame::none()
        .fill(SURFACE_BG)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .rounding(Rounding::ZERO)
        .inner_margin(Margin::same(12.0));

    if let Some(w) = width.into_option_f32() {
        ui.allocate_ui(egui::vec2(w, 0.0), |ui| {
            ui.set_width(w);
            ui.set_min_height(68.0);
            frame.show(ui, add_contents).inner
        })
    } else {
        ui.scope(|ui| {
            ui.set_min_height(68.0);
            frame.show(ui, add_contents).inner
        })
    }
}

/// 渲染现代工业硬核直角主操作按钮 (深冷炭黑底色 / 亮银精致金属边框 / 零圆角)
pub fn primary_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(Color32::from_rgb(26, 32, 44))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(180, 195, 215)))
        .rounding(Rounding::ZERO)
}

/// 渲染危险/中止操作直角按钮 (暗底 / 绯红微弱边框 / 零圆角)
pub fn danger_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(Color32::from_rgb(45, 20, 24))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(180, 50, 60)))
        .rounding(Rounding::ZERO)
}

/// 渲染次级幽灵透明直角按钮 (透明底 / 极细边框 / 零圆角)
pub fn ghost_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .rounding(Rounding::ZERO)
}

/// 渲染置灰禁用直角按钮 (深底 / 暗淡边框 / 零圆角 / 保持一致高度)
pub fn disabled_button(text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::new(text)
        .fill(SURFACE_BG)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .rounding(Rounding::ZERO)
}

/// 渲染置灰禁用直角按钮并强制接入 egui 禁用交互管道 (锁定鼠标禁止光标并屏蔽动画)
pub fn render_disabled_button(
    ui: &mut Ui,
    size: impl Into<Vec2>,
    text: impl Into<WidgetText>,
) -> egui::Response {
    ui.add_enabled_ui(false, |ui| {
        ui.add_sized(size.into(), disabled_button(text))
    })
    .inner
}

/// 渲染微型状态指示点 (Status Marker)
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    
    // 微弱柔和外圈光晕
    let halo_color = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40);
    painter.circle_filled(center, 4.5, halo_color);
    
    // 核心实心小指示点
    painter.circle_filled(center, 3.0, color);
}

/// 渲染极简直角标签 (Tag Badge)
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
    painter.rect_filled(rect, Rounding::ZERO, bg);
    let stroke_color = Color32::from_rgba_unmultiplied(fg.r(), fg.g(), fg.b(), 50);
    painter.rect_stroke(rect, Rounding::ZERO, Stroke::new(1.0_f32, stroke_color));
    painter.galley(rect.min + padding, galley, fg);
}

/// 语义快捷徽标：翠绿合规健康徽标
pub fn badge_green(ui: &mut Ui, text: &str) {
    pill_badge(ui, text, ACCENT_GREEN_BG, ACCENT_GREEN);
}

/// 语义快捷徽标：警示绯红损坏徽标
pub fn badge_red(ui: &mut Ui, text: &str) {
    pill_badge(ui, text, ACCENT_RED_BG, ACCENT_RED);
}

/// 语义快捷徽标：琥珀金黄异常徽标
pub fn badge_amber(ui: &mut Ui, text: &str) {
    pill_badge(ui, text, ACCENT_AMBER_BG, ACCENT_AMBER);
}

/// 语义快捷徽标：微弱冰蓝数据流徽标
pub fn badge_cyan(ui: &mut Ui, text: &str) {
    pill_badge(ui, text, ACCENT_CYAN_BG, ACCENT_CYAN);
}

/// 语义快捷徽标：洋紫覆写假盘徽标
pub fn badge_purple(ui: &mut Ui, text: &str) {
    pill_badge(ui, text, ACCENT_PURPLE_BG, ACCENT_PURPLE);
}

/// 语义快捷徽标：中性沉稳灰度徽标
pub fn badge_neutral(ui: &mut Ui, text: &str) {
    pill_badge(ui, text, SURFACE_BG, TEXT_MUTED);
}
