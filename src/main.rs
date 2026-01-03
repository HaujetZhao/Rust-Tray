#![windows_subsystem = "windows"]

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    TrackPopupMenu, TranslateMessage, AppendMenuW, DestroyMenu, GetSystemMenu, DeleteMenu,
    CW_USEDEFAULT, HICON, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_COMMAND, WM_DESTROY,
    WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_OVERLAPPEDWINDOW, MF_STRING,
    MF_GRAYED, MF_BYCOMMAND, MF_DEFAULT, SW_RESTORE, SW_HIDE,
};
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Console::GetConsoleWindow;
use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, SendMessageW, WM_CLOSE, IsIconic, ShowWindow, MessageBoxW, MB_OK, MB_ICONINFORMATION};

use std::env;
use std::sync::Mutex;

const WM_TRAYICON: u32 = WM_USER + 1;
const IDM_TITLE: u32 = 1001;
const IDM_ABOUT: u32 = 1004;
const IDM_TOGGLE: u32 = 1002;
const IDM_EXIT: u32 = 1003;

const SC_CLOSE: u32 = 0xF060;

// 使用 isize 存储 HWND 以满足 Send + Sync 要求
static PARENT_HWND: Mutex<Option<isize>> = Mutex::new(None);
static TRAY_HWND: Mutex<Option<isize>> = Mutex::new(None);
static APP_NAME: Mutex<Option<String>> = Mutex::new(None);

fn main() {
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE};
        let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
    }

    let args: Vec<String> = env::args().collect();

    // 启动器逻辑：如果是首次启动，则创建后台进程并退出
    if !args.iter().any(|arg| arg == "--detached-child") {
        unsafe {
            use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS, FreeConsole};
            if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
                let parent_hwnd = GetConsoleWindow();
                if !parent_hwnd.0.is_null() {
                    let user_title = if args.len() > 1 { args[1..].join(" ") } else { "Console App".to_string() };
                    let exe_path = env::current_exe().unwrap();
                    
                    use std::process::Command;
                    use std::os::windows::process::CommandExt;
                    const DETACHED_PROCESS: u32 = 0x00000008;

                    let _ = Command::new(exe_path)
                        .arg("--detached-child")
                        .arg(format!("{:p}", parent_hwnd.0))
                        .arg(user_title)
                        .creation_flags(DETACHED_PROCESS)
                        .spawn();
                }
                let _ = FreeConsole();
            }
        }
        return;
    }

    // 后台服务逻辑
    if args.len() < 4 { return; }
    
    let hwnd_str = args[2].trim_start_matches("0x");
    let parent_hwnd_raw = usize::from_str_radix(hwnd_str, 16).unwrap_or(0);
    let app_name = args[3..].join(" ");
    
    *APP_NAME.lock().unwrap() = Some(app_name.clone());

    unsafe {
        let parent_hwnd = HWND(parent_hwnd_raw as *mut _);
        if parent_hwnd.0.is_null() || !windows::Win32::UI::WindowsAndMessaging::IsWindow(parent_hwnd).as_bool() {
            return;
        }

        *PARENT_HWND.lock().unwrap() = Some(parent_hwnd.0 as isize);
        disable_close_button(parent_hwnd);

        let h_instance = GetModuleHandleW(None).unwrap();
        let icon = windows::Win32::UI::WindowsAndMessaging::LoadIconW(h_instance, PCWSTR(1 as _))
            .unwrap_or_else(|_| {
                windows::Win32::UI::WindowsAndMessaging::LoadIconW(None, windows::Win32::UI::WindowsAndMessaging::IDI_APPLICATION).unwrap()
            });

        let parent_raw = parent_hwnd.0 as isize;
        std::thread::spawn(move || {
            monitor_parent_window(parent_raw);
        });

        let tray = TrayManager::new(icon, &app_name);
        tray.run_message_loop();
        tray.destroy();
    }
}

fn disable_close_button(hwnd: HWND) {
    unsafe {
        let h_menu = GetSystemMenu(hwnd, false);
        if !h_menu.is_invalid() {
            let _ = DeleteMenu(h_menu, SC_CLOSE, MF_BYCOMMAND);
        }
    }
}

fn enable_close_button(hwnd: HWND) {
    unsafe {
        let _ = GetSystemMenu(hwnd, true);
    }
}

fn toggle_parent_window() {
    unsafe {
        if let Some(parent_hwnd_raw) = *PARENT_HWND.lock().unwrap() {
            let parent_hwnd = HWND(parent_hwnd_raw as *mut _);
            if IsWindowVisible(parent_hwnd).as_bool() {
                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(parent_hwnd, SW_HIDE);
            } else {
                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(parent_hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(parent_hwnd);
            }
        }
    }
}

fn exit_app() {
    unsafe {
        // 恢复父窗口的关闭按钮
        if let Some(parent_hwnd_raw) = *PARENT_HWND.lock().unwrap() {
            let parent_hwnd = HWND(parent_hwnd_raw as *mut _);
            enable_close_button(parent_hwnd);
            // 发送关闭消息给父窗口
            let _ = SendMessageW(parent_hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        
        // 退出消息循环
        PostQuitMessage(0);
    }
}

fn monitor_parent_window(parent_hwnd_raw: isize) {
    use std::thread;
    use std::time::Duration;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, PostMessageW, WM_CLOSE};
    
    loop {
        thread::sleep(Duration::from_millis(200));
        
        unsafe {
            let parent_hwnd = HWND(parent_hwnd_raw as *mut _);
            
            // 检查窗口是否仍然存在
            if !IsWindow(parent_hwnd).as_bool() {
                // 如果父窗口消失了，告知托盘窗口关闭
                if let Some(tray_hwnd_raw) = *TRAY_HWND.lock().unwrap() {
                    let tray_hwnd = HWND(tray_hwnd_raw as *mut _);
                    let _ = PostMessageW(tray_hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                break;
            }

            // 检查是否被最小化，如果是则隐藏它（实现最小化到托盘）
            if IsWindowVisible(parent_hwnd).as_bool() && IsIconic(parent_hwnd).as_bool() {
                let _ = ShowWindow(parent_hwnd, SW_HIDE);
            }
        }
    }
}


struct TrayManager {
    hwnd: HWND,
}

impl TrayManager {
    fn new(icon: HICON, app_name: &str) -> Self {
        unsafe {
            let h_instance = GetModuleHandleW(None).unwrap();
            let class_name = w!("TrayShieldWindowClass");

            let wnd_class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: h_instance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };

            RegisterClassW(&wnd_class);

            let hwnd = CreateWindowExW(
                Default::default(),
                class_name,
                w!("Tray Shield"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                h_instance,
                None,
            )
            .unwrap();

            *TRAY_HWND.lock().unwrap() = Some(hwnd.0 as isize);

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAYICON,
                hIcon: icon,
                ..Default::default()
            };

            // 设置提示文字
            let tip = format!("{}", app_name);
            let tip_w: Vec<u16> = tip.encode_utf16().chain(Some(0)).collect();
            let len = tip_w.len().min(nid.szTip.len());
            nid.szTip[..len].copy_from_slice(&tip_w[..len]);

            let _ = Shell_NotifyIconW(NIM_ADD, &nid);

            Self { hwnd }
        }
    }

    fn run_message_loop(&self) {
        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn destroy(&self) {
        unsafe {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => match lparam.0 as u32 {
            WM_RBUTTONUP => {
                show_context_menu(hwnd);
                LRESULT(0)
            }
            windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP => {
                toggle_parent_window();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        },
        WM_COMMAND => {
            let id = wparam.0 as u32;
            match id {
                IDM_ABOUT => {
                    show_about_dialog(hwnd);
                }
                IDM_TOGGLE => {
                    toggle_parent_window();
                }
                IDM_EXIT => {
                    exit_app();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap();
    
    // 获取程序名
    let app_name = APP_NAME.lock().unwrap().clone().unwrap_or_else(|| "Console App".to_string());
    let app_name_w: Vec<u16> = app_name.encode_utf16().chain(Some(0)).collect();
    
    // 第一项：程序名（禁用状态）
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_GRAYED,
        IDM_TITLE as usize,
        PCWSTR(app_name_w.as_ptr()),
    );

    // 关于对话框
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        IDM_ABOUT as usize,
        w!("关于..."),
    );

    // 分隔符
    let _ = AppendMenuW(
        menu,
        windows::Win32::UI::WindowsAndMessaging::MF_SEPARATOR,
        0,
        None,
    );
    
    // 第二项：显示/隐藏（默认项）
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_DEFAULT,
        IDM_TOGGLE as usize,
        w!("显示/隐藏"),
    );
    
    // 第三项：退出
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        IDM_EXIT as usize,
        w!("退出"),
    );

    let mut pos = POINT::default();
    GetCursorPos(&mut pos).unwrap();

    // 必须设置前台窗口，否则菜单点击外部不会消失
    let _ = SetForegroundWindow(hwnd);

    let _ = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_BOTTOMALIGN,
        pos.x,
        pos.y,
        0,
        hwnd,
        None,
    );

    let _ = DestroyMenu(menu);
}

unsafe fn show_about_dialog(hwnd: HWND) {
    let about_text = include_str!("../assets/about.txt");
    let h_text = windows::core::HSTRING::from(about_text);
    let _ = MessageBoxW(
        hwnd,
        PCWSTR(h_text.as_ptr()),
        w!("关于 Tray"),
        MB_OK | MB_ICONINFORMATION,
    );
}
