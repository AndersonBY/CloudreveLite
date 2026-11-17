#![windows_subsystem = "windows"]

mod api;
mod config;
mod dlg;
mod files;
mod pipe;
mod progress;
mod tray;
mod ui;
mod update;
mod upload;
mod util;

use std::sync::{Mutex, OnceLock, RwLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_CLOSE,
    WM_DESTROY, WNDCLASSEXW, WNDCLASS_STYLES, WS_POPUP,
};

static CONFIG: RwLock<config::Config> = RwLock::new(config::Config {
    server: String::new(),
    api_key_enc: String::new(),
    expire_days: 7,
    downloads: 10,
    private: false,
    auto_update: true,
});

static LINKS_LAST: Mutex<String> = Mutex::new(String::new());
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

fn is_configured() -> bool {
    !CONFIG.read().unwrap().api_key_enc.is_empty()
}

fn take_boxed_string(lparam: LPARAM) -> String {
    if lparam.0 == 0 {
        return String::new();
    }
    unsafe { *Box::from_raw(lparam.0 as *mut String) }
}

fn handle_menu(hwnd: HWND, cmd: usize) {
    match cmd {
        tray::IDM_SETTINGS => {
            dlg::open(Some(hwnd));
        }
        tray::IDM_AUTOSTART => {
            let now = config::autostart_enabled();
            if !config::set_autostart(!now) {
                tray::balloon(
                    hwnd,
                    "CloudreveLite",
                    "修改开机自启失败，请检查注册表权限。",
                    true,
                );
            }
        }
        tray::IDM_FILES => {
            files::open(Some(hwnd));
        }
        tray::IDM_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if Some(&msg) == TASKBAR_CREATED.get() {
        tray::add(hwnd);
        return LRESULT(0);
    }
    match msg {
        update::WM_UPDATE_EXIT => {
            dlg::close_for_update();
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(hwnd),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER if wparam.0 == 80 => {
            update::tick(hwnd);
            LRESULT(0)
        }
        progress::WM_PROG_BEGIN
        | progress::WM_PROG_FILE
        | progress::WM_PROG_DONE
        | progress::WM_PROG_CANCELLED => {
            progress::handle_msg(hwnd, msg, wparam, lparam);
            LRESULT(0)
        }
        tray::WM_APP_TRAY => {
            // Version 0 semantics: lParam carries the mouse/balloon message.
            let notif = lparam.0 as u32;
            if matches!(notif, 0x0204 | 0x007B | 0x0405 | 0x0202) {
                util::log(&format!("tray msg: notif=0x{notif:04X}"));
            }
            match notif {
                0x0204 /* WM_RBUTTONUP */ | 0x007B /* WM_CONTEXTMENU (keyboard) */ => {
                    let mut pt = windows::Win32::Foundation::POINT { x: 0, y: 0 };
                    let ok = unsafe {
                        windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt)
                    }
                    .is_ok();
                    if ok {
                        let cmd = tray::show_menu(hwnd, pt.x, pt.y);
                        handle_menu(hwnd, cmd);
                    }
                }
                0x0405 /* NIN_BALLOONUSERCLICK */ => {
                    let links = LINKS_LAST.lock().unwrap().clone();
                    if !links.is_empty() {
                        for url in links.lines().take(10) {
                            let u = url.trim();
                            if u.starts_with("http://") || u.starts_with("https://") {
                                util::open_url(u);
                            }
                        }
                    } else if !is_configured() {
                        dlg::open(Some(hwnd));
                    }
                }
                0x0202 /* WM_LBUTTONUP */ => {
                    if !is_configured() {
                        dlg::open(Some(hwnd));
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        upload::WM_APP_UPLOAD_STATE => {
            let text = take_boxed_string(lparam);
            tray::set_tooltip(hwnd, &format!("CloudreveLite：{text}"));
            LRESULT(0)
        }
        upload::WM_APP_UPLOAD_DONE => {
            let text = take_boxed_string(lparam);
            if wparam.0 == 0 {
                tray::balloon(hwnd, "CloudreveLite 上传完成", &text, false);
            } else {
                tray::balloon(hwnd, "CloudreveLite 上传失败", &text, true);
            }
            tray::set_tooltip(hwnd, "CloudreveLite 分享工具");
            LRESULT(0)
        }
        upload::WM_APP_NEED_SETUP => {
            // An upload was requested but no API key is configured: open the
            // settings dialog directly instead of a balloon that is easy to miss.
            tray::balloon(
                hwnd,
                "CloudreveLite",
                take_boxed_string(lparam).trim(),
                true,
            );
            dlg::open(Some(hwnd));
            LRESULT(0)
        }
        upload::WM_APP_UPLOAD_LINKS => {
            let links = take_boxed_string(lparam);
            if !links.is_empty() && util::set_clipboard(&links) {
                *LINKS_LAST.lock().unwrap() = links;
            }
            LRESULT(0)
        }
        WM_CLOSE => unsafe {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        },
        WM_DESTROY => {
            tray::remove(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn another_instance_running() -> bool {
    let name = if std::env::var_os("CLOUDREVE_TRAY_SMOKE_INSTANCE").is_some() {
        "CloudreveLiteTray_Smoke_SingleInstance_Mutex"
    } else {
        "CloudreveLiteTray_SingleInstance_Mutex"
    };
    let name = util::wide(name);
    unsafe {
        let _ = CreateMutexW(None, true, PCWSTR(name.as_ptr()));
        let err = windows::Win32::Foundation::GetLastError();
        err == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
            || err == windows::Win32::Foundation::ERROR_ACCESS_DENIED
    }
}

fn main() {
    ui::init_dpi();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--preview-settings") {
        *CONFIG.write().unwrap() = config::load();
        dlg::open(None);
        return;
    }
    if args.first().map(String::as_str) == Some("--preview-progress") {
        progress::preview();
        return;
    }
    if args.first().map(String::as_str) == Some("--preview-files") {
        files::preview();
        return;
    }
    if !args.is_empty() {
        if !pipe::forward_files(&args) {
            eprintln!("CloudreveLiteTray: 无法将文件发送到托盘进程");
        }
        return;
    }

    util::log_init();
    util::log("starting");

    // Associate the process with an AppUserModelID so tray balloon
    // notifications resolve to a toast on Windows 11.
    unsafe {
        let aumid = util::wide("CloudreveLiteTray");
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(PCWSTR(
            aumid.as_ptr(),
        ));
    }

    if another_instance_running() {
        util::log("another instance is running, exiting");
        return;
    }

    let cfg = config::load();
    let first_run = cfg.api_key_enc.is_empty();
    *CONFIG.write().unwrap() = cfg;

    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let hinst = windows::Win32::Foundation::HINSTANCE(module.0);
        let class_name = util::wide("CloudreveLiteTrayWnd");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: ui::app_icon(false).unwrap_or_default(),
            hCursor: windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
                None,
                windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
            )
            .unwrap_or_default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hIconSm: ui::app_icon(true).unwrap_or_default(),
        };
        let _atom = RegisterClassExW(&wc);

        let title = util::wide("CloudreveLiteTray");
        // WS_POPUP does not support CW_USEDEFAULT; use explicit hidden geometry.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            160,
            160,
            None,
            None,
            Some(hinst),
            None,
        )
        .expect("create window failed");

        let tbc = util::wide("TaskbarCreated");
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(PCWSTR(tbc.as_ptr())));

        if !tray::add(hwnd) {
            util::log("failed to add tray icon");
        }

        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        // HWND is a plain handle value; carry it across threads as usize and
        // rebuild it where PostMessageW is called.
        let hwnd_worker = hwnd.0 as usize;
        let hwnd_pipe = hwnd.0 as usize;
        std::thread::spawn(move || upload::worker_static(HWND(hwnd_worker as *mut _), rx));
        std::thread::spawn(move || pipe::server(HWND(hwnd_pipe as *mut _), tx));

        windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), 80, 1000, None);

        if first_run {
            tray::balloon(
                hwnd,
                "CloudreveLite",
                "首次使用：点击本气泡或左键点击托盘图标打开设置，用网页端的配对码连接服务器。",
                false,
            );
            dlg::open(Some(hwnd));
        }

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        util::log("exiting");
    }
}
