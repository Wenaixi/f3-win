// Win32 原生系统托盘驱动与后台消息循环模块
// 基于 Windows Shell_NotifyIconW 与 TrackPopupMenu 实现，零额外三方依赖

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use eframe::egui;

use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage,
    CW_USEDEFAULT, HCURSOR, HICON, HMENU, IDI_APPLICATION, MB_ICONINFORMATION, MB_OK,
    MF_SEPARATOR, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_COMMAND, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_USER,
    WNDCLASSW, WNDCLASS_STYLES,
};

/// 托盘向主界面派发的交互事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// 唤醒并聚焦主窗口
    ShowWindow,
    /// 弹出路径全景信息对话框
    ShowPathInfoModal,
    /// 唤起关于应用信息对话框
    ShowAboutModal,
    /// 在资源管理器中打开可执行程序所在目录
    OpenExeDir,
    /// 在资源管理器中打开当前选中的测试盘符
    OpenSelectedVolume,
    /// 在资源管理器中打开维权存证档案目录
    OpenEvidenceDir,
    /// 请求退出应用程序 (若有测试在运行需二次确认)
    RequestExit,
}

/// 主界面向托盘派发的控制指令
#[derive(Debug, Clone)]
pub enum TrayControlCmd {
    /// 动态更新托盘图标悬停提示文案
    UpdateTooltip(String),
    /// 移除托盘图标并退出后台消息循环
    Destroy,
}

const WM_TRAY_CALLBACK: u32 = WM_USER + 2048;
const WM_TRAY_CMD: u32 = WM_USER + 2049;

const IDM_SHOW_WINDOW: usize = 1001;
const IDM_PATH_INFO: usize = 1002;
const IDM_OPEN_EXE_DIR: usize = 1003;
const IDM_OPEN_VOLUME_DIR: usize = 1004;
const IDM_OPEN_EVIDENCE_DIR: usize = 1005;
const IDM_EXIT: usize = 1006;
const IDM_ABOUT: usize = 1007;

struct TrayWorkerState {
    hwnd: isize,
    event_tx: Sender<TrayEvent>,
    cmd_rx: Receiver<TrayControlCmd>,
    taskbar_created_msg: u32,
    current_tip: String,
    ctx: egui::Context,
}

static TRAY_STATE: Mutex<Option<TrayWorkerState>> = Mutex::new(None);
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAY_CALLBACK {
        let mouse_msg = (lparam.0 & 0xFFFF) as u32;
        if mouse_msg == WM_LBUTTONUP || mouse_msg == WM_LBUTTONDBLCLK {
            if let Some(ref state) = *TRAY_STATE.lock() {
                state.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                state.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                let _ = state.event_tx.send(TrayEvent::ShowWindow);
                state.ctx.request_repaint();
            }
            return LRESULT(0);
        } else if mouse_msg == WM_RBUTTONUP {
            show_tray_context_menu(hwnd);
            return LRESULT(0);
        }
    } else if msg == WM_COMMAND {
        let cmd_id = wparam.0 & 0xFFFF;
        if cmd_id == IDM_ABOUT {
            let version = env!("CARGO_PKG_VERSION");
            let text_owned = format!(
                "F3-Win v{}\nFight Flash Fraud for Windows\n\n开源闪存真伪检测与自愈工具\nDirect I/O 原生高性能驱动架构\nhttps://github.com/Wenaixi/f3-win\0",
                version
            );
            // 异步解耦弹窗，绝不挂起托盘主消息循环，防止退出时 join() 发生死锁
            std::thread::spawn(move || {
                let text_wide: Vec<u16> = text_owned.encode_utf16().collect();
                let title = w!("关于 F3-Win");
                unsafe {
                    let _ = MessageBoxW(
                        HWND::default(),
                        PCWSTR(text_wide.as_ptr()),
                        title,
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            });
            if let Some(ref state) = *TRAY_STATE.lock() {
                state.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                state.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                let _ = state.event_tx.send(TrayEvent::ShowAboutModal);
                state.ctx.request_repaint();
            }
            return LRESULT(0);
        }

        let event = match cmd_id {
            IDM_SHOW_WINDOW => {
                if let Some(ref state) = *TRAY_STATE.lock() {
                    state.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    state.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Some(TrayEvent::ShowWindow)
            }
            IDM_PATH_INFO => {
                if let Some(ref state) = *TRAY_STATE.lock() {
                    state.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    state.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Some(TrayEvent::ShowPathInfoModal)
            }
            IDM_OPEN_EXE_DIR => Some(TrayEvent::OpenExeDir),
            IDM_OPEN_VOLUME_DIR => Some(TrayEvent::OpenSelectedVolume),
            IDM_OPEN_EVIDENCE_DIR => Some(TrayEvent::OpenEvidenceDir),
            IDM_EXIT => {
                if let Some(ref state) = *TRAY_STATE.lock() {
                    state.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
                Some(TrayEvent::RequestExit)
            }
            _ => None,
        };
        if let Some(ev) = event {
            if let Some(ref state) = *TRAY_STATE.lock() {
                let _ = state.event_tx.send(ev);
                state.ctx.request_repaint();
            }
        }
        return LRESULT(0);
    } else if msg == WM_TRAY_CMD {
        let mut should_destroy = false;
        {
            if let Some(ref mut state) = *TRAY_STATE.lock() {
                while let Ok(cmd) = state.cmd_rx.try_recv() {
                    match cmd {
                        TrayControlCmd::UpdateTooltip(new_tip) => {
                            if state.current_tip != new_tip {
                                state.current_tip = new_tip.clone();
                                modify_tray_tooltip(hwnd, &new_tip);
                            }
                        }
                        TrayControlCmd::Destroy => {
                            should_destroy = true;
                            break;
                        }
                    }
                }
            }
        }

        if should_destroy {
            remove_tray_icon(hwnd);
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
        }
        return LRESULT(0);
    } else if msg == WM_DESTROY {
        remove_tray_icon(hwnd);
        PostQuitMessage(0);
        return LRESULT(0);
    } else {
        let should_restore = {
            if let Some(ref state) = *TRAY_STATE.lock() {
                state.taskbar_created_msg != 0 && msg == state.taskbar_created_msg
            } else {
                false
            }
        };

        if should_restore {
            let tip = {
                TRAY_STATE
                    .lock()
                    .as_ref()
                    .map(|s| s.current_tip.clone())
                    .unwrap_or_else(|| "F3 (管理员) - 闪存完整性测试".to_string())
            };
            add_tray_icon(hwnd, &tip);
            return LRESULT(0);
        }
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn show_tray_context_menu(hwnd: HWND) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);

    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => return,
    };

    let _ = AppendMenuW(menu, MF_STRING, IDM_SHOW_WINDOW, w!("显示主窗口"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, IDM_PATH_INFO, w!("查看路径信息..."));
    let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN_EXE_DIR, w!("打开程序目录"));
    let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN_VOLUME_DIR, w!("打开当前测试盘符"));
    let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN_EVIDENCE_DIR, w!("打开证据归档目录"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let version = env!("CARGO_PKG_VERSION");
    let about_label = format!("关于 F3-Win (v{})\0", version);
    let about_wide: Vec<u16> = about_label.encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING, IDM_ABOUT, PCWSTR(about_wide.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("安全退出 (Exit)"));

    let _ = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );

    let _ = DestroyMenu(menu);
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
}

/// 加载系统托盘品牌图标 (三级容灾自适应架构)
unsafe fn load_tray_icon() -> HICON {
    // 1. 优先尝试从 PE 资源中加载 1 号主图标 (由 winres 编译嵌入，支持多分辨率与系统 DPI 自动适配)
    if let Ok(hmodule) = windows::Win32::System::LibraryLoader::GetModuleHandleW(None) {
        let h_instance = HINSTANCE(hmodule.0);
        if let Ok(icon) = LoadIconW(h_instance, PCWSTR(1 as _)) {
            if !icon.is_invalid() {
                return icon;
            }
        }
    }

    // 2. 次级保底: 直接从静态编译期内嵌的 32x32 极简品牌 PNG 资源中动态构建 HICON
    static EMBEDDED_ICON_PNG: &[u8] = include_bytes!("../../assets/icon_32.png");
    if let Ok(icon) = windows::Win32::UI::WindowsAndMessaging::CreateIconFromResourceEx(
        EMBEDDED_ICON_PNG,
        true,
        0x00030000,
        0,
        0,
        windows::Win32::UI::WindowsAndMessaging::LR_DEFAULTCOLOR,
    ) {
        if !icon.is_invalid() {
            return icon;
        }
    }

    // 3. 终极兜底: 使用 Windows 系统默认应用程序图标
    LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
}

unsafe fn make_notify_icon_data(hwnd: HWND, tooltip: &str) -> NOTIFYICONDATAW {
    let mut nid = std::mem::zeroed::<NOTIFYICONDATAW>();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_CALLBACK;
    nid.hIcon = load_tray_icon();

    let tip_u16: Vec<u16> = tooltip.encode_utf16().collect();
    let len = tip_u16.len().min(127);
    nid.szTip[..len].copy_from_slice(&tip_u16[..len]);
    nid
}

unsafe fn add_tray_icon(hwnd: HWND, tooltip: &str) -> bool {
    let nid = make_notify_icon_data(hwnd, tooltip);
    Shell_NotifyIconW(NIM_ADD, &nid).as_bool()
}

unsafe fn modify_tray_tooltip(hwnd: HWND, tooltip: &str) -> bool {
    let mut nid = std::mem::zeroed::<NOTIFYICONDATAW>();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_TIP;
    let tip_u16: Vec<u16> = tooltip.encode_utf16().collect();
    let len = tip_u16.len().min(127);
    nid.szTip[..len].copy_from_slice(&tip_u16[..len]);
    Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool()
}

unsafe fn remove_tray_icon(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    let mut nid = std::mem::zeroed::<NOTIFYICONDATAW>();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    Shell_NotifyIconW(NIM_DELETE, &nid).as_bool()
}

/// 系统托盘管理器实例 (持有后台线程句柄与通信管道)
pub struct TrayManager {
    thread_handle: Option<thread::JoinHandle<()>>,
    cmd_tx: Sender<TrayControlCmd>,
    event_rx: Receiver<TrayEvent>,
    hwnd: isize,
}

impl TrayManager {
    /// 启动 Win32 原生托盘后台服务
    pub fn new(initial_tooltip: &str, ctx: egui::Context) -> Self {
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<TrayEvent>();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<TrayControlCmd>();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<(isize, Result<(), String>)>(1);

        let tooltip_owned = initial_tooltip.to_string();

        let thread_handle = thread::Builder::new()
            .name("f3-win-tray".to_string())
            .spawn(move || {
                unsafe {
                    let class_name = w!("F3WinTrayMessageWindow");
                    if !CLASS_REGISTERED.swap(true, Ordering::SeqCst) {
                        let wc = WNDCLASSW {
                            style: WNDCLASS_STYLES(0),
                            lpfnWndProc: Some(tray_wnd_proc),
                            cbClsExtra: 0,
                            cbWndExtra: 0,
                            hInstance: HINSTANCE(std::ptr::null_mut()),
                            hIcon: HICON::default(),
                            hCursor: HCURSOR::default(),
                            hbrBackground: HBRUSH(std::ptr::null_mut()),
                            lpszMenuName: PCWSTR::null(),
                            lpszClassName: class_name,
                        };
                        RegisterClassW(&wc);
                    }

                    // 创建顶级不可见窗口作为宿主 (hWndParent 为 NULL)，满足 Shell 广播与前景窗口规范
                    let hwnd_res = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        class_name,
                        w!("F3WinTray"),
                        WINDOW_STYLE(0),
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        HWND::default(),
                        HMENU::default(),
                        HINSTANCE(std::ptr::null_mut()),
                        None,
                    );

                    let hwnd = match hwnd_res {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = ready_tx.send((0, Err(format!("创建托盘消息窗口失败: {}", e))));
                            return;
                        }
                    };

                    let taskbar_msg = RegisterWindowMessageW(w!("TaskbarCreated"));

                    add_tray_icon(hwnd, &tooltip_owned);

                    {
                        *TRAY_STATE.lock() = Some(TrayWorkerState {
                            hwnd: hwnd.0 as isize,
                            event_tx,
                            cmd_rx,
                            taskbar_created_msg: taskbar_msg,
                            current_tip: tooltip_owned,
                            ctx,
                        });
                    }

                    let _ = ready_tx.send((hwnd.0 as isize, Ok(())));

                    let mut msg = MSG::default();
                    // 严格使用 .0 > 0 判定：防范 GetMessageW 出错返回 -1 导致的 100% CPU 死循环
                    while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }

                    // 线程退出时释放全局状态
                    *TRAY_STATE.lock() = None;
                }
            })
            .ok();

        let hwnd = match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok((h, Ok(()))) => h,
            _ => 0,
        };

        Self {
            thread_handle,
            cmd_tx,
            event_rx,
            hwnd,
        }
    }

    /// 尝试拉取当前未消费的托盘事件
    pub fn try_recv_event(&self) -> Option<TrayEvent> {
        self.event_rx.try_recv().ok()
    }

    /// 动态更新托盘图标的悬停提示文案
    pub fn set_tooltip(&self, text: &str) {
        if self.hwnd != 0 {
            let _ = self.cmd_tx.send(TrayControlCmd::UpdateTooltip(text.to_string()));
            unsafe {
                let _ = PostMessageW(
                    HWND(self.hwnd as *mut core::ffi::c_void),
                    WM_TRAY_CMD,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    /// 销毁系统托盘并安全退出托盘线程
    pub fn destroy(&mut self) {
        if self.hwnd != 0 {
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            unsafe {
                // 双重防御：在调用端主线程立即移除托盘图标，彻底根除幽灵残留
                remove_tray_icon(hwnd);
                let _ = self.cmd_tx.send(TrayControlCmd::Destroy);
                let _ = PostMessageW(
                    hwnd,
                    WM_TRAY_CMD,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            self.hwnd = 0;
        }

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TrayManager {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tray_event_equality() {
        assert_eq!(TrayEvent::ShowWindow, TrayEvent::ShowWindow);
        assert_ne!(TrayEvent::ShowWindow, TrayEvent::RequestExit);
        assert_eq!(TrayEvent::ShowPathInfoModal, TrayEvent::ShowPathInfoModal);
        assert_eq!(TrayEvent::ShowAboutModal, TrayEvent::ShowAboutModal);
    }

    #[test]
    fn test_tray_control_cmd_debug() {
        let cmd = TrayControlCmd::UpdateTooltip("测试提示".to_string());
        assert!(format!("{:?}", cmd).contains("测试提示"));
    }

    #[test]
    fn test_tray_event_channel_communication() {
        let (tx, rx) = crossbeam_channel::unbounded::<TrayEvent>();
        tx.send(TrayEvent::RequestExit).unwrap();
        tx.send(TrayEvent::ShowWindow).unwrap();
        tx.send(TrayEvent::ShowPathInfoModal).unwrap();
        tx.send(TrayEvent::OpenExeDir).unwrap();
        tx.send(TrayEvent::OpenSelectedVolume).unwrap();
        tx.send(TrayEvent::OpenEvidenceDir).unwrap();

        assert_eq!(rx.recv().unwrap(), TrayEvent::RequestExit);
        assert_eq!(rx.recv().unwrap(), TrayEvent::ShowWindow);
        assert_eq!(rx.recv().unwrap(), TrayEvent::ShowPathInfoModal);
        assert_eq!(rx.recv().unwrap(), TrayEvent::OpenExeDir);
        assert_eq!(rx.recv().unwrap(), TrayEvent::OpenSelectedVolume);
        assert_eq!(rx.recv().unwrap(), TrayEvent::OpenEvidenceDir);
    }

    #[test]
    fn test_tray_control_cmd_channel() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<TrayControlCmd>();
        cmd_tx.send(TrayControlCmd::UpdateTooltip("测试提示".to_string())).unwrap();
        cmd_tx.send(TrayControlCmd::Destroy).unwrap();

        match cmd_rx.recv().unwrap() {
            TrayControlCmd::UpdateTooltip(t) => assert_eq!(t, "测试提示"),
            _ => panic!("预期 UpdateTooltip 指令"),
        }
        match cmd_rx.recv().unwrap() {
            TrayControlCmd::Destroy => {}
            _ => panic!("预期 Destroy 指令"),
        }
    }
}
