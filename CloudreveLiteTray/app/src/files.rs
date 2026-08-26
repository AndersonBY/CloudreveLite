//! Small native file browser for the tray menu.
//!
//! It deliberately renders only text rows through the Win32 list-view control:
//! no embedded browser, thumbnails, or resident polling are needed.

use crate::{api, config, ui, util};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{SetBkMode, SetTextColor, HDC, TRANSPARENT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_SELECTED, LVITEMW,
    LVM_DELETEALLITEMS, LVM_GETITEMSTATE, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
    LVS_EX_HEADERDRAGDROP, LVS_REPORT, LVS_SHOWSELALWAYS, NMLISTVIEW, NM_CLICK, NM_DBLCLK,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_FILES_RESULT: u32 = WM_APP + 40;
pub const WM_APP_FILES_DELETE_RESULT: u32 = WM_APP + 41;

const IDC_BACK: i32 = 3001;
const IDC_REFRESH: i32 = 3002;
const IDC_LIST: i32 = 3003;
const IDC_PATH: i32 = 3004;
const IDC_STATUS: i32 = 3005;
const IDC_TITLE: i32 = 3006;
const IDC_DELETE: i32 = 3007;
const ROOT_URI: &str = "cloudreve://my/";
const WND_CLASS: &str = "CloudreveLiteFilesWnd";

static WND: AtomicIsize = AtomicIsize::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
static CURRENT_URI: Mutex<String> = Mutex::new(String::new());
static ENTRIES: Mutex<Vec<api::FileEntry>> = Mutex::new(Vec::new());

struct FileLoadResult {
    uri: String,
    result: Result<Vec<api::FileEntry>, String>,
}

struct FileDeleteResult {
    result: Result<(), String>,
}

fn hwnd() -> Option<HWND> {
    let value = WND.load(Ordering::Relaxed);
    (!value.eq(&0)).then(|| HWND(value as *mut _))
}

pub fn open(owner: Option<HWND>) {
    if let Some(existing) = hwnd() {
        unsafe {
            let _ = ShowWindow(existing, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(existing);
        }
        return;
    }

    unsafe { create_window(owner, true) };
}

pub fn preview() {
    unsafe {
        create_window(None, false);
        if let Some(window) = hwnd() {
            apply_entries(
                window,
                vec![
                    api::FileEntry {
                        file_type: 1,
                        name: "项目资料".into(),
                        path: "cloudreve://my/项目资料/".into(),
                        size: 0,
                        updated_at: "2026-08-26T14:20:00+08:00".into(),
                    },
                    api::FileEntry {
                        file_type: 0,
                        name: "CloudreveLite 使用说明.pdf".into(),
                        path: "cloudreve://my/CloudreveLite%20使用说明.pdf".into(),
                        size: 2_457_600,
                        updated_at: "2026-08-25T09:12:00+08:00".into(),
                    },
                    api::FileEntry {
                        file_type: 0,
                        name: "BladeSlice-win-x64.zip".into(),
                        path: "cloudreve://my/BladeSlice-win-x64.zip".into(),
                        size: 187_700_000,
                        updated_at: "2026-08-22T18:06:00+08:00".into(),
                    },
                ],
            );
            set_path_text(window, ROOT_URI);
            set_status(window, "共 3 项");
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe fn create_window(owner: Option<HWND>, load: bool) {
    let module = GetModuleHandleW(None).unwrap_or_default();
    let hinst = windows::Win32::Foundation::HINSTANCE(module.0);
    let class_name = util::wide(WND_CLASS);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hIcon: ui::app_icon(false).unwrap_or_default(),
        hIconSm: ui::app_icon(true).unwrap_or_default(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: ui::background_brush(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    let _ = RegisterClassExW(&wc);

    let title = util::wide("CloudreveLite 文件管理");
    let window = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_THICKFRAME,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        ui::scale(720),
        ui::scale(520),
        owner,
        None,
        Some(hinst),
        None,
    )
    .expect("file window creation failed");
    WND.store(window.0 as isize, Ordering::Relaxed);
    ui::set_dpi(GetDpiForWindow(window));
    ui::apply_light_frame(window);
    create_controls(window, hinst);
    layout(window);
    set_path_text(window, ROOT_URI);
    set_status(window, "正在加载文件列表…");
    let _ = ShowWindow(window, SW_SHOWNORMAL);
    let _ = SetForegroundWindow(window);
    if load {
        request_load(window, ROOT_URI.to_string());
    }
}

unsafe fn create_controls(window: HWND, hinst: windows::Win32::Foundation::HINSTANCE) {
    let body_font = ui::font(-15, 400);
    let path_font = ui::font(-16, 600);
    let button_font = ui::font(-14, 400);
    let mk = |class: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: i32, text: &str| {
        let control = CreateWindowExW(
            ex,
            PCWSTR(util::wide(class).as_ptr()),
            PCWSTR(util::wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            Some(window),
            Some(HMENU(id as *mut _)),
            Some(hinst),
            None,
        )
        .expect("file control creation failed");
        ui::theme(control);
        control
    };

    let title = mk(
        "STATIC",
        WINDOW_STYLE(0),
        WINDOW_EX_STYLE(0),
        IDC_TITLE,
        "我的文件",
    );
    let path = mk(
        "STATIC",
        WINDOW_STYLE(0),
        WINDOW_EX_STYLE(0),
        IDC_PATH,
        "正在准备文件列表…",
    );
    let status = mk(
        "STATIC",
        WINDOW_STYLE(0),
        WINDOW_EX_STYLE(0),
        IDC_STATUS,
        "",
    );
    let back = mk(
        "BUTTON",
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_EX_STYLE(0),
        IDC_BACK,
        "返回上级",
    );
    let refresh = mk(
        "BUTTON",
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_EX_STYLE(0),
        IDC_REFRESH,
        "刷新",
    );
    let delete = mk(
        "BUTTON",
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_EX_STYLE(0),
        IDC_DELETE,
        "删除",
    );
    let list = mk(
        "SysListView32",
        WS_TABSTOP | WINDOW_STYLE(LVS_REPORT as u32 | LVS_SHOWSELALWAYS as u32),
        WS_EX_CLIENTEDGE,
        IDC_LIST,
        "",
    );

    for (control, font) in [
        (title, path_font),
        (path, body_font),
        (status, body_font),
        (back, button_font),
        (refresh, button_font),
        (delete, button_font),
        (list, body_font),
    ] {
        ui::apply_font(control, font);
    }

    let _ = SendMessageW(
        list,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        Some(WPARAM(0)),
        Some(LPARAM(
            (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP) as isize,
        )),
    );
    insert_column(list, 0, "名称", 300);
    insert_column(list, 1, "类型", 90);
    insert_column(list, 2, "大小", 100);
    insert_column(list, 3, "修改时间", 130);
}

unsafe fn insert_column(list: HWND, index: i32, label: &str, width: i32) {
    let text = util::wide(label);
    let mut column = LVCOLUMNW {
        mask: LVCF_TEXT | LVCF_WIDTH,
        cx: ui::scale(width),
        pszText: PWSTR(text.as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = SendMessageW(
        list,
        LVM_INSERTCOLUMNW,
        Some(WPARAM(index as usize)),
        Some(LPARAM(&mut column as *mut _ as isize)),
    );
}

unsafe fn layout(window: HWND) {
    let mut rect = RECT::default();
    let _ = GetClientRect(window, &mut rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let margin = ui::scale(20);
    let top = ui::scale(18);
    let button_w = ui::scale(82);
    let button_h = ui::scale(30);
    let gap = ui::scale(8);
    let title_h = ui::scale(28);
    let subtitle_h = ui::scale(22);
    let status_h = ui::scale(22);
    let list_top = top + title_h + subtitle_h + ui::scale(16);
    let list_bottom = height - margin - status_h;

    move_control(
        window,
        IDC_TITLE,
        margin,
        top,
        width - margin * 2 - button_w * 3 - gap * 2,
        title_h,
    );
    move_control(
        window,
        IDC_PATH,
        margin,
        top + title_h,
        width - margin * 2,
        subtitle_h,
    );
    move_control(
        window,
        IDC_BACK,
        width - margin - button_w * 3 - gap * 2,
        top,
        button_w,
        button_h,
    );
    move_control(
        window,
        IDC_REFRESH,
        width - margin - button_w * 2 - gap,
        top,
        button_w,
        button_h,
    );
    move_control(
        window,
        IDC_DELETE,
        width - margin - button_w,
        top,
        button_w,
        button_h,
    );
    move_control(
        window,
        IDC_LIST,
        margin,
        list_top,
        width - margin * 2,
        (list_bottom - list_top).max(ui::scale(80)),
    );
    if let Ok(list) = GetDlgItem(Some(window), IDC_LIST) {
        let list_width = (width - margin * 2 - ui::scale(4)).max(ui::scale(360));
        set_column_width(list, 0, list_width * 44 / 100);
        set_column_width(list, 1, list_width * 16 / 100);
        set_column_width(list, 2, list_width * 14 / 100);
        set_column_width(list, 3, list_width * 26 / 100);
    }
    move_control(
        window,
        IDC_STATUS,
        margin,
        height - margin - status_h,
        width - margin * 2,
        status_h,
    );
}

unsafe fn move_control(window: HWND, id: i32, x: i32, y: i32, width: i32, height: i32) {
    if let Ok(control) = GetDlgItem(Some(window), id) {
        let _ = MoveWindow(control, x, y, width.max(0), height.max(0), true);
    }
}

unsafe fn set_column_width(list: HWND, column: usize, width: i32) {
    let _ = SendMessageW(
        list,
        LVM_SETCOLUMNWIDTH,
        Some(WPARAM(column)),
        Some(LPARAM(width.max(ui::scale(60)) as isize)),
    );
}

fn request_load(window: HWND, uri: String) {
    set_busy(window, true);
    *CURRENT_URI.lock().unwrap() = uri.clone();
    set_path_text(window, &uri);
    set_status(window, "正在加载文件列表…");
    let (server, key) = {
        let cfg = crate::CONFIG.read().unwrap().clone();
        (cfg.server, config::decrypt_key(&cfg.api_key_enc))
    };
    let window_value = window.0 as usize;
    std::thread::spawn(move || {
        let result = if key.is_empty() {
            Err("尚未连接，请先在设置中完成配对。".to_string())
        } else {
            api::list_files(&server, &key, &uri).map_err(|error| error.msg)
        };
        let payload = Box::new(FileLoadResult { uri, result });
        unsafe {
            let payload = Box::into_raw(payload);
            if PostMessageW(
                Some(HWND(window_value as *mut _)),
                WM_APP_FILES_RESULT,
                WPARAM(0),
                LPARAM(payload as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(payload));
            }
        }
    });
}

fn request_delete(window: HWND, uris: Vec<String>) {
    set_busy(window, true);
    set_status(window, "正在移入回收站…");
    let (server, key) = {
        let cfg = crate::CONFIG.read().unwrap().clone();
        (cfg.server, config::decrypt_key(&cfg.api_key_enc))
    };
    let window_value = window.0 as usize;
    std::thread::spawn(move || {
        let result = if key.is_empty() {
            Err("尚未连接，请先在设置中完成配对。".to_string())
        } else {
            api::delete_files(&server, &key, &uris).map_err(|error| error.msg)
        };
        let payload = Box::into_raw(Box::new(FileDeleteResult { result }));
        unsafe {
            if PostMessageW(
                Some(HWND(window_value as *mut _)),
                WM_APP_FILES_DELETE_RESULT,
                WPARAM(0),
                LPARAM(payload as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(payload));
            }
        }
    });
}

fn set_busy(window: HWND, busy: bool) {
    BUSY.store(busy, Ordering::Relaxed);
    unsafe {
        for id in [IDC_BACK, IDC_REFRESH] {
            if let Ok(control) = GetDlgItem(Some(window), id) {
                let _ = EnableWindow(control, !busy);
            }
        }
        let has_selection = !busy && !selected_uris(window).is_empty();
        if let Ok(control) = GetDlgItem(Some(window), IDC_DELETE) {
            let _ = EnableWindow(control, has_selection);
        }
    }
}

unsafe fn selected_uris(window: HWND) -> Vec<String> {
    let list = GetDlgItem(Some(window), IDC_LIST).unwrap_or_default();
    let entries = ENTRIES.lock().unwrap().clone();
    let mut selected = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let state = SendMessageW(
            list,
            LVM_GETITEMSTATE,
            Some(WPARAM(index)),
            Some(LPARAM(LVIS_SELECTED.0 as isize)),
        )
        .0 as u32;
        if state & LVIS_SELECTED.0 != 0 {
            selected.push(entry.path.clone());
        }
    }
    selected
}

fn set_path_text(window: HWND, uri: &str) {
    let text = format!("我的文件  /  {}", display_path(uri));
    unsafe {
        let _ = SetDlgItemTextW(window, IDC_PATH, PCWSTR(util::wide(&text).as_ptr()));
    }
}

fn set_status(window: HWND, text: &str) {
    unsafe {
        let _ = SetDlgItemTextW(window, IDC_STATUS, PCWSTR(util::wide(text).as_ptr()));
    }
}

fn display_path(uri: &str) -> String {
    let path = uri
        .strip_prefix(ROOT_URI)
        .unwrap_or(uri)
        .trim_end_matches('/');
    if path.is_empty() {
        "根目录".into()
    } else {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }
}

fn parent_uri(uri: &str) -> Option<String> {
    let current = uri.trim_end_matches('/');
    if current == ROOT_URI.trim_end_matches('/') {
        return None;
    }
    let index = current.rfind('/')?;
    let parent = &current[..=index];
    Some(if parent == "cloudreve://my/" {
        ROOT_URI.to_string()
    } else {
        parent.to_string()
    })
}

unsafe fn apply_entries(window: HWND, entries: Vec<api::FileEntry>) {
    set_busy(window, false);
    *ENTRIES.lock().unwrap() = entries.clone();
    let list = GetDlgItem(Some(window), IDC_LIST).unwrap_or_default();
    let _ = SendMessageW(list, LVM_DELETEALLITEMS, Some(WPARAM(0)), Some(LPARAM(0)));
    for (index, entry) in entries.iter().enumerate() {
        let name = util::wide(&entry.name);
        let mut item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: index as i32,
            pszText: PWSTR(name.as_ptr() as *mut _),
            ..Default::default()
        };
        let _ = SendMessageW(
            list,
            LVM_INSERTITEMW,
            Some(WPARAM(0)),
            Some(LPARAM(&mut item as *mut _ as isize)),
        );
        set_item_text(
            list,
            index,
            1,
            if entry.file_type == 1 {
                "文件夹"
            } else {
                "文件"
            },
        );
        set_item_text(
            list,
            index,
            2,
            if entry.file_type == 1 {
                "—".into()
            } else {
                format_size(entry.size)
            },
        );
        set_item_text(list, index, 3, format_time(&entry.updated_at));
    }
    if entries.is_empty() {
        set_status(window, "当前文件夹为空");
    } else {
        set_status(
            window,
            &format!("共 {} 项 · 按住 Ctrl 或 Shift 可多选", entries.len()),
        );
    }
}

unsafe fn set_item_text(list: HWND, row: usize, column: i32, text: impl Into<String>) {
    let text = util::wide(&text.into());
    let mut item = LVITEMW {
        iSubItem: column,
        pszText: PWSTR(text.as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = SendMessageW(
        list,
        LVM_SETITEMTEXTW,
        Some(WPARAM(row)),
        Some(LPARAM(&mut item as *mut _ as isize)),
    );
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        return format!("{size} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = "B";
    for next in units {
        value /= 1024.0;
        unit = next;
        if value < 1024.0 {
            break;
        }
    }
    if value >= 100.0 {
        format!("{value:.0} {unit}")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn format_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

unsafe extern "system" fn wndproc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SIZE => {
            layout(window);
            LRESULT(0)
        }
        WM_COMMAND => {
            if BUSY.load(Ordering::Relaxed) {
                return LRESULT(0);
            }
            match (wparam.0 & 0xffff) as i32 {
                IDC_REFRESH => {
                    let uri = CURRENT_URI.lock().unwrap().clone();
                    request_load(window, uri);
                }
                IDC_BACK => {
                    let current = CURRENT_URI.lock().unwrap().clone();
                    if let Some(parent) = parent_uri(&current) {
                        request_load(window, parent);
                    }
                }
                IDC_DELETE => {
                    let uris = selected_uris(window);
                    if uris.is_empty() {
                        set_status(window, "请先选择要删除的文件或文件夹");
                    } else {
                        let prompt = format!(
                            "确定要删除选中的 {} 项吗？\n文件会先移入回收站。",
                            uris.len()
                        );
                        let answer = MessageBoxW(
                            Some(window),
                            PCWSTR(util::wide(&prompt).as_ptr()),
                            PCWSTR(util::wide("确认删除").as_ptr()),
                            MB_ICONWARNING | MB_YESNO | MB_DEFBUTTON2,
                        );
                        if answer == IDYES {
                            request_delete(window, uris);
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DRAWITEM => {
            if lparam.0 != 0 {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                match item.CtlID as i32 {
                    IDC_REFRESH => ui::draw_button(item, true),
                    IDC_DELETE => ui::draw_danger_button(item),
                    IDC_BACK => ui::draw_button(item, false),
                    _ => return LRESULT(0),
                }
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut _);
            let control = HWND(lparam.0 as *mut _);
            let color = if GetDlgCtrlID(control) == IDC_TITLE {
                ui::TEXT
            } else {
                ui::MUTED
            };
            let _ = SetTextColor(hdc, color);
            let _ = SetBkMode(hdc, TRANSPARENT);
            LRESULT(ui::background_brush().0 as isize)
        }
        WM_NOTIFY => {
            if lparam.0 != 0 {
                let notify = &*(lparam.0 as *const NMLISTVIEW);
                if notify.hdr.code == NM_CLICK && notify.iItem >= 0 {
                    if let Some(entry) = ENTRIES.lock().unwrap().get(notify.iItem as usize) {
                        set_status(window, &format!("已选择：{}", entry.name));
                    }
                    let has_selection = !selected_uris(window).is_empty();
                    if let Ok(control) = GetDlgItem(Some(window), IDC_DELETE) {
                        let _ = EnableWindow(control, has_selection);
                    }
                } else if notify.hdr.code == NM_DBLCLK && notify.iItem >= 0 {
                    if let Some(entry) = ENTRIES.lock().unwrap().get(notify.iItem as usize).cloned()
                    {
                        if entry.file_type == 1 {
                            request_load(window, entry.path);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_FILES_RESULT => {
            if lparam.0 != 0 {
                let payload = *Box::from_raw(lparam.0 as *mut FileLoadResult);
                if payload.uri == *CURRENT_URI.lock().unwrap() {
                    match payload.result {
                        Ok(entries) => apply_entries(window, entries),
                        Err(error) => {
                            set_busy(window, false);
                            set_status(window, &error);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_FILES_DELETE_RESULT => {
            if lparam.0 != 0 {
                let payload = *Box::from_raw(lparam.0 as *mut FileDeleteResult);
                match payload.result {
                    Ok(()) => {
                        set_status(window, "删除完成，正在刷新…");
                        let uri = CURRENT_URI.lock().unwrap().clone();
                        request_load(window, uri);
                    }
                    Err(error) => {
                        set_busy(window, false);
                        set_status(window, &format!("删除失败：{error}"));
                    }
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(window);
            LRESULT(0)
        }
        WM_DESTROY => {
            WND.store(0, Ordering::Relaxed);
            BUSY.store(false, Ordering::Relaxed);
            *ENTRIES.lock().unwrap() = Vec::new();
            LRESULT(0)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}
