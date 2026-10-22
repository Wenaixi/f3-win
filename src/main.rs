#![allow(dead_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod gui;

use eframe::egui::ViewportBuilder;
use gui::app::F3App;
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_NORMAL;

fn ensure_administrator_elevation() {
    unsafe {
        if IsUserAnAdmin().as_bool() {
            return;
        }

        // 若当前未以管理员权限启动，自动通过 runas 重新唤起管理员进程并退出当前受限进程
        if let Ok(current_exe) = std::env::current_exe() {
            let mut exe_wide: Vec<u16> = current_exe.to_string_lossy().encode_utf16().collect();
            exe_wide.push(0);

            let mut runas_wide: Vec<u16> = "runas".encode_utf16().collect();
            runas_wide.push(0);

            let ret = ShellExecuteW(
                None,
                PCWSTR(runas_wide.as_ptr()),
                PCWSTR(exe_wide.as_ptr()),
                None,
                None,
                SW_NORMAL,
            );

            if ret.0 as usize > 32 {
                std::process::exit(0);
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    ensure_administrator_elevation();

    // 内嵌 64x64 极简品牌像素矩阵，零运行时解码依赖与开销
    let icon_data = eframe::egui::IconData {
        rgba: include_bytes!("../assets/icon_64.rgba").to_vec(),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([920.0, 660.0])
            .with_min_inner_size([800.0, 540.0])
            .with_title("F3 (管理员)")
            .with_icon(icon_data),
        ..Default::default()
    };

    eframe::run_native(
        "F3-Win",
        options,
        Box::new(|cc| Ok(Box::new(F3App::new(cc)))),
    )
}
