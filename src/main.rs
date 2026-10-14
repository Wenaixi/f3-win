#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod gui;

use eframe::egui::ViewportBuilder;
use gui::app::F3App;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([960.0, 700.0])
            .with_min_inner_size([820.0, 580.0])
            .with_title("F3-Win 极致闪存测试与自愈终端"),
        ..Default::default()
    };

    eframe::run_native(
        "F3-Win",
        options,
        Box::new(|cc| Ok(Box::new(F3App::new(cc)))),
    )
}
