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
            let mut exe_wide: Vec<u16> = current_exe.as_os_str().encode_utf16().collect();
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

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([960.0, 700.0])
            .with_min_inner_size([820.0, 580.0])
            .with_title("F3-Win 极致闪存测试与自愈终端 (管理员特权)"),
        ..Default::default()
    };

    eframe::run_native(
        "F3-Win",
        options,
        Box::new(|cc| Ok(Box::new(F3App::new(cc)))),
    )
}
