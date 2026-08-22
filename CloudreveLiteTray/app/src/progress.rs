//! Lightweight upload progress window.
//!
//! The worker thread publishes progress through atomics (bytes sent, cancel
//! flag); the main-thread window polls them on a timer, so no cross-thread
//! message storm is needed. State transitions (file start / done / error /
//! cancelled) arrive as posted messages carrying heap-allocated payloads.

use crate::{ui, util};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi;
use windows::Win32::Graphics::Gdi::{FillRect, SetBkMode, SetTextColor, TRANSPARENT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, DRAWITEMSTRUCT, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX, PBM_SETPOS,
    PBM_SETRANGE32, PBS_SMOOTH,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_PROG_BEGIN: u32 = WM_APP + 20;
pub const WM_PROG_FILE: u32 = WM_APP + 21;
pub const WM_PROG_DONE: u32 = WM_APP + 22;
pub const WM_PROG_CANCELLED: u32 = WM_APP + 24;

const IDCANCEL_BTN: i32 = 1001;
const IDC_PROGBAR: i32 = 1002;
const IDC_TITLE: i32 = 1003;
const IDC_FILE: i32 = 1004;
const IDC_META: i32 = 1005;
const TIMER_POLL: usize = 1;
const TIMER_AUTOCLOSE: usize = 2;

static SENT: AtomicU64 = AtomicU64::new(0);
static CANCEL: AtomicBool = AtomicBool::new(false);
static WND: AtomicIsize = AtomicIsize::new(0);
static VISUAL_STATE: AtomicU64 = AtomicU64::new(0); // 0 uploading, 1 success, 2 error
static PREVIEW: AtomicBool = AtomicBool::new(false);

pub fn add_sent(n: u64) {
    SENT.fetch_add(n, Ordering::Relaxed);
}

pub fn sent() -> u64 {
    SENT.load(Ordering::Relaxed)
}

pub fn set_sent(n: u64) {
    SENT.store(n, Ordering::Relaxed);
}

pub fn cancel_requested() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

fn request_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

fn reset_state() {
    CANCEL.store(false, Ordering::Relaxed);
    SENT.store(0, Ordering::Relaxed);
    PREVIEW.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------- main-side ---

/// Per-file snapshot the window keeps for the polling timer.
struct FileState {
    index: usize,
    total_files: usize,
    size: u64,
    name: String,
    last_sent: u64,
    last_tick: std::time::Instant,
    finished: bool,
}

static FILE_STATE: std::sync::Mutex<Option<FileState>> = std::sync::Mutex::new(None);

pub fn handle_msg(hwnd_main: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) {
    match msg {
        WM_PROG_BEGIN => {
            reset_state();
            show_window(hwnd_main);
        }
        WM_PROG_FILE => {
            let boxed = unsafe { Box::from_raw(lparam.0 as *mut (usize, usize, u64, String)) };
            let (index, total_files, size, name) = *boxed;
            set_sent(0);
            *FILE_STATE.lock().unwrap() = Some(FileState {
                index,
                total_files,
                size,
                name,
                last_sent: 0,
                last_tick: std::time::Instant::now(),
                finished: false,
            });
            VISUAL_STATE.store(0, Ordering::Relaxed);
            if WND.load(Ordering::Relaxed) != 0 {
                show_window(hwnd_main);
            }
        }
        WM_PROG_DONE => {
            let text = take_boxed_string(lparam);
            let ok = wparam.0 == 0;
            finish(hwnd_main, ok, &text);
        }
        WM_PROG_CANCELLED => {
            finish(hwnd_main, false, "已取消上传。");
        }
        _ => {}
    }
}

fn take_boxed_string(lparam: LPARAM) -> String {
    if lparam.0 == 0 {
        String::new()
    } else {
        unsafe { *Box::from_raw(lparam.0 as *mut String) }
    }
}

fn finish(_hwnd_main: HWND, ok: bool, text: &str) {
    let hwnd = HWND(WND.load(Ordering::Relaxed) as *mut _);
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_POLL);
        VISUAL_STATE.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
        set_text(
            hwnd,
            IDC_TITLE,
            if ok {
                "分享链接已复制"
            } else {
                "上传没有完成"
            },
        );
        set_text(
            hwnd,
            IDC_FILE,
            if ok {
                "文件已上传，并创建了临时分享链接。"
            } else {
                "请根据下面的信息处理后重试。"
            },
        );
        set_text(hwnd, IDC_META, text);
        SendDlgItemMessageW(
            hwnd,
            IDC_PROGBAR,
            PBM_SETPOS,
            WPARAM(if ok { 100 } else { 0 }),
            LPARAM(0),
        );
        set_text(hwnd, IDCANCEL_BTN, "关闭");
        *FILE_STATE.lock().unwrap() = None;
        // auto-close success after 4s; errors stay until dismissed
        if ok {
            SetTimer(Some(hwnd), TIMER_AUTOCLOSE, 4000, None);
        }
    }
}

fn set_text(hwnd: HWND, id: i32, s: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id, windows::core::PCWSTR(util::wide(s).as_ptr()));
    }
}

// ------------------------------------------------------------------ window ---

const WND_CLASS: &str = "CloudreveLiteProgressWnd";

pub fn show_window(hwnd_main: HWND) {
    let existing = WND.load(Ordering::Relaxed);
    if existing != 0 {
        unsafe {
            let hwnd = HWND(existing as *mut _);
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
            SetTimer(Some(hwnd), TIMER_POLL, 250, None);
        }
        return;
    }
    unsafe { create_window(hwnd_main) };
}

unsafe fn create_window(hwnd_main: HWND) {
    let ictl = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_PROGRESS_CLASS,
    };
    let _ = InitCommonControlsEx(&ictl);

    let module = GetModuleHandleW(None).unwrap_or_default();
    let hinst = windows::Win32::Foundation::HINSTANCE(module.0);
    let class_name = util::wide(WND_CLASS);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: ui::background_brush(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);

    // place near the bottom-right of the primary monitor work area
    let mut wa = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut wa as *mut _ as *mut _),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    let w = ui::scale(500);
    let h = ui::scale(238);
    let x = wa.right - w - ui::scale(24);
    let y = wa.bottom - h - ui::scale(24);

    let title = util::wide("CloudreveLite 上传");
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP | WS_CAPTION | WS_SYSMENU,
        x,
        y,
        w,
        h,
        Some(hwnd_main),
        None,
        Some(hinst),
        None,
    )
    .expect("progress window creation failed");
    WND.store(hwnd.0 as isize, Ordering::Relaxed);

    ui::set_dpi(GetDpiForWindow(hwnd));
    let w = ui::scale(500);
    let h = ui::scale(238);
    let x = wa.right - w - ui::scale(24);
    let y = wa.bottom - h - ui::scale(24);
    let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);

    ui::apply_light_frame(hwnd);

    let title_font = ui::font(-22, 600);
    let body_font = ui::font(-15, 400);
    let meta_font = ui::font(-14, 400);
    let mk = |class: &str,
              style: WINDOW_STYLE,
              ex: WINDOW_EX_STYLE,
              id: i32,
              xx: i32,
              yy: i32,
              ww: i32,
              hh: i32,
              text: &str| {
        let hwnd_ctl = CreateWindowExW(
            ex,
            PCWSTR(util::wide(class).as_ptr()),
            PCWSTR(util::wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            ui::scale(xx),
            ui::scale(yy),
            ui::scale(ww),
            ui::scale(hh),
            Some(hwnd),
            Some(windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _)),
            Some(hinst),
            None,
        )
        .expect("control creation failed");
        ui::theme(hwnd_ctl);
        hwnd_ctl
    };
    let title_ctl = mk(
        "STATIC",
        WINDOW_STYLE(0),
        WINDOW_EX_STYLE(0),
        IDC_TITLE,
        28,
        22,
        424,
        30,
        "准备上传",
    );
    let file_ctl = mk(
        "STATIC",
        WINDOW_STYLE(0),
        WINDOW_EX_STYLE(0),
        IDC_FILE,
        28,
        57,
        424,
        24,
        "正在建立上传任务…",
    );
    let progress_ctl = mk(
        "msctls_progress32",
        WINDOW_STYLE(PBS_SMOOTH),
        WINDOW_EX_STYLE(0),
        IDC_PROGBAR,
        28,
        91,
        424,
        12,
        "",
    );
    let meta_ctl = mk(
        "STATIC",
        WINDOW_STYLE(0),
        WINDOW_EX_STYLE(0),
        IDC_META,
        28,
        116,
        424,
        42,
        "等待服务器响应…",
    );
    let cancel_ctl = mk(
        "BUTTON",
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_EX_STYLE(0),
        IDCANCEL_BTN,
        354,
        166,
        98,
        32,
        "取消",
    );

    ui::apply_font(title_ctl, title_font);
    ui::apply_font(file_ctl, body_font);
    ui::apply_font(meta_ctl, meta_font);
    ui::apply_font(cancel_ctl, body_font);
    ui::theme(progress_ctl);

    unsafe {
        SendDlgItemMessageW(hwnd, IDC_PROGBAR, PBM_SETRANGE32, WPARAM(0), LPARAM(100));
    }
    let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
    let _ = SetForegroundWindow(hwnd);
    SetTimer(Some(hwnd), TIMER_POLL, 250, None);
}

pub unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            if (wparam.0 & 0xFFFF) as i32 == IDCANCEL_BTN {
                on_close_or_cancel(hwnd);
            }
            LRESULT(0)
        }
        WM_DRAWITEM => {
            if lparam.0 != 0 {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if item.CtlID as i32 == IDCANCEL_BTN {
                    ui::draw_button(item, false);
                    return LRESULT(1);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            on_close_or_cancel(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_POLL {
                poll(hwnd);
            } else if wparam.0 == TIMER_AUTOCLOSE {
                close_window(hwnd);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let hdc = Gdi::HDC(wparam.0 as *mut _) as _;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let _ = FillRect(hdc, &rc, ui::background_brush());
            LRESULT(1)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = Gdi::HDC(wparam.0 as *mut _);
            let control = HWND(lparam.0 as *mut _);
            let id = GetDlgCtrlID(control);
            let color = if id == IDC_TITLE {
                match VISUAL_STATE.load(Ordering::Relaxed) {
                    1 => ui::SUCCESS,
                    2 => ui::ERROR,
                    _ => ui::ACCENT,
                }
            } else if id == IDC_META {
                ui::MUTED
            } else {
                ui::TEXT
            };
            let _ = SetTextColor(hdc, color);
            let _ = SetBkMode(hdc, TRANSPARENT);
            LRESULT(ui::background_brush().0 as isize)
        }
        WM_DESTROY => {
            WND.store(0, Ordering::Relaxed);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn on_close_or_cancel(hwnd: HWND) {
    // while uploading, X / cancel requests cancellation and the window waits
    // for the worker to confirm; after finish it just closes.
    let finished = PREVIEW.load(Ordering::Relaxed) || FILE_STATE.lock().unwrap().is_none();
    if finished {
        close_window(hwnd);
    } else {
        request_cancel();
        set_text(hwnd, IDC_TITLE, "正在取消上传");
        set_text(hwnd, IDC_META, "正在结束当前上传，请稍候…");
    }
}

fn close_window(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_POLL);
        let _ = KillTimer(Some(hwnd), TIMER_AUTOCLOSE);
        let _ = DestroyWindow(hwnd);
    }
    WND.store(0, Ordering::Relaxed);
}

fn poll(hwnd: HWND) {
    let sent = SENT.load(Ordering::Relaxed);
    let mut guard = FILE_STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return };
    if st.finished {
        return;
    }
    let now = std::time::Instant::now();
    let dt = now.duration_since(st.last_tick).as_secs_f64();
    let speed = if dt >= 0.2 && sent >= st.last_sent {
        (sent - st.last_sent) as f64 / dt
    } else {
        0.0
    };
    if dt >= 0.25 {
        st.last_sent = sent;
        st.last_tick = now;
    }
    let pct = if st.size > 0 {
        ((sent as f64 / st.size as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        100.0
    };
    let file_line = format!(
        "{} / {}  {}",
        st.index,
        st.total_files,
        shorten_name(&st.name, 46)
    );
    let meta_line = if speed > 0.0 && pct < 100.0 {
        format!(
            "{:.0}%  ·  {} / {}  ·  {}/s  ·  剩余约 {}",
            pct,
            human_bytes(sent as f64),
            human_bytes(st.size as f64),
            human_bytes(speed),
            human_time(((st.size - sent) as f64 / speed).max(0.0) as u64)
        )
    } else if pct >= 100.0 {
        "文件已传输，正在创建分享链接…".to_string()
    } else {
        format!(
            "{:.0}%  ·  {} / {}",
            pct,
            human_bytes(sent as f64),
            human_bytes(st.size as f64)
        )
    };
    unsafe {
        set_text(hwnd, IDC_TITLE, "正在上传");
        set_text(hwnd, IDC_FILE, &file_line);
        set_text(hwnd, IDC_META, &meta_line);
        SendDlgItemMessageW(
            hwnd,
            IDC_PROGBAR,
            PBM_SETPOS,
            WPARAM(pct as usize),
            LPARAM(0),
        );
    }
}

pub fn preview() {
    reset_state();
    PREVIEW.store(true, Ordering::Relaxed);
    let size = 128 * 1024 * 1024u64;
    set_sent(58 * 1024 * 1024);
    *FILE_STATE.lock().unwrap() = Some(FileState {
        index: 2,
        total_files: 4,
        size,
        name: "CloudreveLite 产品演示与交付资料 2026-08-22.zip".into(),
        last_sent: 58 * 1024 * 1024,
        last_tick: std::time::Instant::now(),
        finished: false,
    });
    show_window(HWND::default());
    unsafe {
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
            if WND.load(Ordering::Relaxed) == 0 {
                break;
            }
        }
    }
}

fn shorten_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        return name.to_string();
    }
    let head = max_chars.saturating_sub(12);
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - 11..].iter().collect::<String>()
    )
}

fn human_bytes(b: f64) -> String {
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{:.0} B", b)
    }
}

fn human_time(s: u64) -> String {
    if s >= 3600 {
        format!("{}时{}分", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}分{}秒", s / 60, s % 60)
    } else {
        format!("{}秒", s)
    }
}
