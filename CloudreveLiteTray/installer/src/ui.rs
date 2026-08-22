use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, FrameRect, GetStockObject, InvalidateRect, RoundRect, SelectObject, SetBkMode,
    SetTextColor, UpdateWindow, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    DEFAULT_QUALITY, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBRUSH, HFONT, NULL_BRUSH,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, DRAWITEMSTRUCT, ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX,
    ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, PBM_SETPOS, PBM_SETRANGE32,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Install,
    Uninstall,
}

type Work = Box<dyn FnOnce() + Send + 'static>;

const IDC_TITLE: i32 = 1101;
const IDC_SUBTITLE: i32 = 1102;
const IDC_STATUS: i32 = 1103;
const IDC_DETAIL: i32 = 1104;
const IDC_PROGRESS: i32 = 1105;
const IDC_FOOTNOTE: i32 = 1106;
const IDC_PRIMARY: i32 = 1107;
const IDC_SECONDARY: i32 = 1108;

const WM_UI_STATUS: u32 = WM_APP + 70;
const WM_UI_NOTE: u32 = WM_APP + 71;
const WM_UI_FINISH: u32 = WM_APP + 72;

const STATE_IDLE: usize = 0;
const STATE_RUNNING: usize = 1;
const STATE_FINISHED: usize = 2;

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static MODE: AtomicUsize = AtomicUsize::new(0);
static STATE: AtomicUsize = AtomicUsize::new(STATE_IDLE);
static PROGRESS: AtomicUsize = AtomicUsize::new(8);
static WORK: OnceLock<Mutex<Option<Work>>> = OnceLock::new();
static FONT_TITLE: AtomicIsize = AtomicIsize::new(0);
static FONT_BODY: AtomicIsize = AtomicIsize::new(0);
static FONT_STRONG: AtomicIsize = AtomicIsize::new(0);
static BACKGROUND: AtomicIsize = AtomicIsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static DPI: AtomicUsize = AtomicUsize::new(96);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | ((g as u32) << 8) | ((b as u32) << 16))
}

pub fn init_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        set_dpi(GetDpiForSystem());
    }
}

fn set_dpi(dpi: u32) {
    DPI.store(dpi.max(96) as usize, Ordering::Relaxed);
}

fn scale(value: i32) -> i32 {
    let dpi = DPI.load(Ordering::Relaxed) as i32;
    if value < 0 {
        -((-value * dpi + 48) / 96)
    } else {
        (value * dpi + 48) / 96
    }
}

fn post_string(message: u32, wparam: usize, text: &str) {
    let hwnd = HWND(WINDOW.load(Ordering::Relaxed) as *mut _);
    if hwnd.0.is_null() {
        return;
    }
    let payload = Box::into_raw(Box::new(text.to_string())) as isize;
    unsafe {
        if PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(payload)).is_err() {
            drop(Box::from_raw(payload as *mut String));
        }
    }
}

pub fn step(text: &str) {
    let next = PROGRESS
        .fetch_add(11, Ordering::Relaxed)
        .saturating_add(11)
        .min(92);
    post_string(WM_UI_STATUS, next, text);
}

pub fn note(text: &str) {
    post_string(WM_UI_NOTE, 0, text);
}

pub fn finish(ok: bool, text: &str) {
    post_string(WM_UI_FINISH, usize::from(ok), text);
}

pub fn show_error(text: &str) {
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(wide(text).as_ptr()),
            PCWSTR(wide("CloudreveLite 安装程序").as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn take_string(lparam: LPARAM) -> String {
    if lparam.0 == 0 {
        String::new()
    } else {
        unsafe { *Box::from_raw(lparam.0 as *mut String) }
    }
}

fn current_mode() -> Mode {
    if MODE.load(Ordering::Relaxed) == 1 {
        Mode::Uninstall
    } else {
        Mode::Install
    }
}

unsafe fn set_text(hwnd: HWND, id: i32, text: &str) {
    let _ = SetDlgItemTextW(hwnd, id, PCWSTR(wide(text).as_ptr()));
}

unsafe fn set_font(hwnd: HWND, id: i32, font: HFONT) {
    let control = GetDlgItem(Some(hwnd), id).unwrap_or_default();
    if !control.0.is_null() {
        let _ = SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

unsafe fn create_font(height: i32, weight: i32) -> HFONT {
    CreateFontW(
        scale(height),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        DEFAULT_QUALITY,
        DEFAULT_PITCH.0 as u32,
        PCWSTR(wide("Segoe UI Variable Text").as_ptr()),
    )
}

unsafe fn start_work(hwnd: HWND) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    STATE.store(STATE_RUNNING, Ordering::Relaxed);
    PROGRESS.store(8, Ordering::Relaxed);
    let mode = current_mode();
    set_text(
        hwnd,
        IDC_STATUS,
        if mode == Mode::Install {
            "正在准备安装…"
        } else {
            "正在准备卸载…"
        },
    );
    set_text(hwnd, IDC_DETAIL, "请稍候，操作进行期间不要关闭此窗口。");
    SendDlgItemMessageW(hwnd, IDC_PROGRESS, PBM_SETPOS, WPARAM(8), LPARAM(0));
    let _ = EnableWindow(
        GetDlgItem(Some(hwnd), IDC_PRIMARY).unwrap_or_default(),
        false,
    );
    let _ = EnableWindow(
        GetDlgItem(Some(hwnd), IDC_SECONDARY).unwrap_or_default(),
        false,
    );

    if let Some(work) = WORK.get().and_then(|slot| slot.lock().ok()?.take()) {
        std::thread::spawn(work);
    } else {
        finish(false, "安装任务未能启动，请关闭窗口后重试。");
    }
}

unsafe fn create_controls(hwnd: HWND) {
    let module = GetModuleHandleW(None).unwrap_or_default();
    let hinst = windows::Win32::Foundation::HINSTANCE(module.0);
    let mk = |class: &str,
              text: &str,
              style: WINDOW_STYLE,
              id: i32,
              x: i32,
              y: i32,
              width: i32,
              height: i32| {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide(class).as_ptr()),
            PCWSTR(wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            scale(x),
            scale(y),
            scale(width),
            scale(height),
            Some(hwnd),
            Some(HMENU(id as *mut _)),
            Some(hinst),
            None,
        )
        .unwrap_or_default()
    };

    let mode = current_mode();
    mk(
        "STATIC",
        if mode == Mode::Install {
            "安装 CloudreveLite 分享工具"
        } else {
            "卸载 CloudreveLite 分享工具"
        },
        WINDOW_STYLE(0),
        IDC_TITLE,
        88,
        24,
        420,
        32,
    );
    mk(
        "STATIC",
        if mode == Mode::Install {
            "从资源管理器右键上传文件，并立即创建临时分享链接。"
        } else {
            "移除托盘程序和右键菜单，保留你的连接配置。"
        },
        WINDOW_STYLE(0),
        IDC_SUBTITLE,
        88,
        58,
        420,
        22,
    );
    mk(
        "STATIC",
        "准备就绪",
        WINDOW_STYLE(0),
        IDC_STATUS,
        50,
        126,
        438,
        26,
    );
    mk(
        "STATIC",
        if mode == Mode::Install {
            "将安装到当前 Windows 用户。Windows 11 现代右键菜单可能需要一次系统确认。"
        } else {
            "托盘程序、右键菜单和开始菜单入口将被移除；用户配置与日志会保留。"
        },
        WINDOW_STYLE(0),
        IDC_DETAIL,
        50,
        156,
        438,
        44,
    );
    let progress = mk(
        "msctls_progress32",
        "",
        WINDOW_STYLE(0),
        IDC_PROGRESS,
        50,
        211,
        438,
        12,
    );
    mk(
        "STATIC",
        if mode == Mode::Install {
            "安装位置：当前用户的本地应用目录"
        } else {
            "你的服务器地址与 API Key 不会被删除"
        },
        WINDOW_STYLE(0),
        IDC_FOOTNOTE,
        50,
        231,
        438,
        20,
    );
    let primary = mk(
        "BUTTON",
        if mode == Mode::Install {
            "开始安装"
        } else {
            "开始卸载"
        },
        WS_TABSTOP | WINDOW_STYLE((BS_DEFPUSHBUTTON | BS_OWNERDRAW) as u32),
        IDC_PRIMARY,
        358,
        287,
        130,
        36,
    );
    let secondary = mk(
        "BUTTON",
        "取消",
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        IDC_SECONDARY,
        258,
        287,
        88,
        36,
    );

    SendDlgItemMessageW(hwnd, IDC_PROGRESS, PBM_SETRANGE32, WPARAM(0), LPARAM(100));
    SendDlgItemMessageW(hwnd, IDC_PROGRESS, PBM_SETPOS, WPARAM(0), LPARAM(0));
    let explorer = wide("Explorer");
    let _ = SetWindowTheme(progress, PCWSTR(explorer.as_ptr()), PCWSTR::null());
    let _ = SetWindowTheme(primary, PCWSTR(explorer.as_ptr()), PCWSTR::null());
    let _ = SetWindowTheme(secondary, PCWSTR(explorer.as_ptr()), PCWSTR::null());

    let title = create_font(-24, 600);
    let body = create_font(-15, 400);
    let strong = create_font(-17, 600);
    FONT_TITLE.store(title.0 as isize, Ordering::Relaxed);
    FONT_BODY.store(body.0 as isize, Ordering::Relaxed);
    FONT_STRONG.store(strong.0 as isize, Ordering::Relaxed);
    set_font(hwnd, IDC_TITLE, title);
    set_font(hwnd, IDC_STATUS, strong);
    for id in [
        IDC_SUBTITLE,
        IDC_DETAIL,
        IDC_FOOTNOTE,
        IDC_PRIMARY,
        IDC_SECONDARY,
    ] {
        set_font(hwnd, id, body);
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);

    let background = CreateSolidBrush(rgb(248, 249, 251));
    let _ = FillRect(hdc, &client, background);
    let accent = CreateSolidBrush(rgb(36, 99, 235));
    let _ = FillRect(
        hdc,
        &RECT {
            left: scale(0),
            top: scale(0),
            right: scale(8),
            bottom: client.bottom,
        },
        accent,
    );
    let panel = CreateSolidBrush(rgb(255, 255, 255));
    let panel_rect = RECT {
        left: scale(28),
        top: scale(108),
        right: scale(516),
        bottom: scale(260),
    };
    let _ = FillRect(hdc, &panel_rect, panel);
    let border = CreateSolidBrush(rgb(222, 226, 234));
    let _ = FrameRect(hdc, &panel_rect, border);

    let module = GetModuleHandleW(None).unwrap_or_default();
    let icon_size = scale(44);
    if let Ok(handle) = LoadImageW(
        Some(windows::Win32::Foundation::HINSTANCE(module.0)),
        PCWSTR(101 as *const u16),
        IMAGE_ICON,
        icon_size,
        icon_size,
        LR_DEFAULTCOLOR | LR_SHARED,
    ) {
        let icon = HICON(handle.0);
        let _ = DrawIconEx(
            hdc,
            scale(28),
            scale(25),
            icon,
            icon_size,
            icon_size,
            0,
            None,
            DI_NORMAL,
        );
    }

    let _ = DeleteObject(background.into());
    let _ = DeleteObject(accent.into());
    let _ = DeleteObject(panel.into());
    let _ = DeleteObject(border.into());
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn draw_button(item: &DRAWITEMSTRUCT, primary: bool) {
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
    let (fill, border, text) = if disabled {
        (rgb(229, 233, 239), rgb(213, 219, 227), rgb(144, 153, 168))
    } else if primary {
        if selected {
            (rgb(29, 78, 216), rgb(29, 78, 216), rgb(255, 255, 255))
        } else {
            (rgb(37, 99, 235), rgb(37, 99, 235), rgb(255, 255, 255))
        }
    } else if selected {
        (rgb(236, 240, 246), rgb(177, 187, 201), rgb(24, 32, 45))
    } else {
        (rgb(255, 255, 255), rgb(190, 199, 212), rgb(24, 32, 45))
    };
    let brush = CreateSolidBrush(fill);
    let pen = CreatePen(PS_SOLID, 1, border);
    let old_brush = SelectObject(item.hDC, brush.into());
    let old_pen = SelectObject(item.hDC, pen.into());
    let mut rect = item.rcItem;
    if selected {
        rect.top += 1;
        rect.bottom += 1;
    }
    let _ = RoundRect(
        item.hDC,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        scale(10),
        scale(10),
    );
    if focused && !disabled {
        let focus_pen = CreatePen(
            PS_SOLID,
            1,
            if primary {
                rgb(255, 255, 255)
            } else {
                rgb(37, 99, 235)
            },
        );
        let old_focus_pen = SelectObject(item.hDC, focus_pen.into());
        let old_focus_brush = SelectObject(item.hDC, GetStockObject(NULL_BRUSH));
        let _ = RoundRect(
            item.hDC,
            rect.left + scale(3),
            rect.top + scale(3),
            rect.right - scale(3),
            rect.bottom - scale(3),
            scale(8),
            scale(8),
        );
        let _ = SelectObject(item.hDC, old_focus_brush);
        let _ = SelectObject(item.hDC, old_focus_pen);
        let _ = DeleteObject(focus_pen.into());
    }
    let font = SendMessageW(item.hwndItem, WM_GETFONT, None, None);
    let old_font = if font.0 != 0 {
        Some(SelectObject(
            item.hDC,
            windows::Win32::Graphics::Gdi::HGDIOBJ(font.0 as *mut _),
        ))
    } else {
        None
    };
    let _ = SetBkMode(item.hDC, TRANSPARENT);
    let _ = SetTextColor(item.hDC, text);
    let mut label = [0u16; 128];
    let length = GetWindowTextW(item.hwndItem, &mut label).max(0) as usize;
    if length > 0 {
        let _ = DrawTextW(
            item.hDC,
            &mut label[..length],
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    if let Some(old) = old_font {
        let _ = SelectObject(item.hDC, old);
    }
    let _ = SelectObject(item.hDC, old_brush);
    let _ = SelectObject(item.hDC, old_pen);
    let _ = DeleteObject(brush.into());
    let _ = DeleteObject(pen.into());
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            let control = HWND(lparam.0 as *mut _);
            let id = GetDlgCtrlID(control);
            let color = match id {
                IDC_TITLE | IDC_STATUS => rgb(24, 32, 45),
                IDC_SUBTITLE | IDC_DETAIL => rgb(74, 85, 104),
                _ => rgb(107, 117, 134),
            };
            let _ = SetTextColor(hdc, color);
            let _ = SetBkMode(hdc, TRANSPARENT);
            LRESULT(GetStockObject(WHITE_BRUSH).0 as isize)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            if id == IDC_PRIMARY {
                if STATE.load(Ordering::Relaxed) == STATE_IDLE {
                    start_work(hwnd);
                } else if STATE.load(Ordering::Relaxed) == STATE_FINISHED {
                    let _ = DestroyWindow(hwnd);
                }
            } else if id == IDC_SECONDARY && STATE.load(Ordering::Relaxed) != STATE_RUNNING {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DRAWITEM => {
            if lparam.0 != 0 {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if item.CtlID as i32 == IDC_PRIMARY || item.CtlID as i32 == IDC_SECONDARY {
                    draw_button(item, item.CtlID as i32 == IDC_PRIMARY);
                    return LRESULT(1);
                }
            }
            LRESULT(0)
        }
        WM_UI_STATUS => {
            let text = take_string(lparam);
            set_text(hwnd, IDC_STATUS, &text);
            SendDlgItemMessageW(
                hwnd,
                IDC_PROGRESS,
                PBM_SETPOS,
                WPARAM(wparam.0.min(92)),
                LPARAM(0),
            );
            LRESULT(0)
        }
        WM_UI_NOTE => {
            set_text(hwnd, IDC_DETAIL, &take_string(lparam));
            LRESULT(0)
        }
        WM_UI_FINISH => {
            let ok = wparam.0 != 0;
            let text = take_string(lparam);
            RUNNING.store(false, Ordering::Relaxed);
            STATE.store(STATE_FINISHED, Ordering::Relaxed);
            set_text(
                hwnd,
                IDC_STATUS,
                if ok {
                    if current_mode() == Mode::Install {
                        "安装完成"
                    } else {
                        "卸载完成"
                    }
                } else if current_mode() == Mode::Install {
                    "安装没有完成"
                } else {
                    "卸载没有完成"
                },
            );
            set_text(hwnd, IDC_DETAIL, &text);
            SendDlgItemMessageW(
                hwnd,
                IDC_PROGRESS,
                PBM_SETPOS,
                WPARAM(if ok { 100 } else { 0 }),
                LPARAM(0),
            );
            set_text(hwnd, IDC_PRIMARY, "完成");
            let primary = GetDlgItem(Some(hwnd), IDC_PRIMARY).unwrap_or_default();
            let secondary = GetDlgItem(Some(hwnd), IDC_SECONDARY).unwrap_or_default();
            let _ = EnableWindow(primary, true);
            let _ = ShowWindow(secondary, SW_HIDE);
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_CLOSE => {
            if STATE.load(Ordering::Relaxed) == STATE_RUNNING {
                set_text(
                    hwnd,
                    IDC_DETAIL,
                    "当前操作正在进行，请等待它完成后再关闭窗口。",
                );
            } else {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            WINDOW.store(0, Ordering::Relaxed);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub fn run<F>(mode: Mode, work: F)
where
    F: FnOnce() + Send + 'static,
{
    MODE.store(usize::from(mode == Mode::Uninstall), Ordering::Relaxed);
    STATE.store(STATE_IDLE, Ordering::Relaxed);
    PROGRESS.store(8, Ordering::Relaxed);
    RUNNING.store(false, Ordering::Relaxed);
    let _ = WORK.set(Mutex::new(Some(Box::new(work))));

    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_PROGRESS_CLASS,
        };
        let _ = InitCommonControlsEx(&controls);

        let module = GetModuleHandleW(None).unwrap_or_default();
        let hinst = windows::Win32::Foundation::HINSTANCE(module.0);
        let class_name = wide("CloudreveLiteSetupWindow");
        let background = CreateSolidBrush(rgb(248, 249, 251));
        BACKGROUND.store(background.0 as isize, Ordering::Relaxed);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: LoadIconW(Some(hinst), PCWSTR(101 as *const u16)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(background.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);

        let width = scale(560);
        let height = scale(390);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(
                wide(if mode == Mode::Install {
                    "安装 CloudreveLite"
                } else {
                    "卸载 CloudreveLite"
                })
                .as_ptr(),
            ),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            (screen_w - width) / 2,
            (screen_h - height) / 2,
            width,
            height,
            None,
            None,
            Some(hinst),
            None,
        )
        .unwrap_or_default();
        if hwnd.0.is_null() {
            show_error("无法创建安装窗口，请重启 Windows 后重试。");
            return;
        }

        set_dpi(GetDpiForWindow(hwnd));
        let width = scale(560);
        let height = scale(390);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let _ = SetWindowPos(
            hwnd,
            None,
            (screen_w - width) / 2,
            (screen_h - height) / 2,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        create_controls(hwnd);

        let caption = rgb(248, 249, 251);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption as *const _ as *const _,
            4,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetForegroundWindow(hwnd);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
