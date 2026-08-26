use crate::util;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, SetForegroundWindow, TrackPopupMenuEx, HICON, HMENU,
    MENU_ITEM_FLAGS, MF_CHECKED, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
};

pub const WM_APP_TRAY: u32 = WM_APP;

pub const IDM_SETTINGS: usize = 2001;
pub const IDM_AUTOSTART: usize = 2002;
pub const IDM_FILES: usize = 2003;
pub const IDM_QUIT: usize = 2004;

pub const TRAY_UID: u32 = 1;

fn load_tray_icon() -> Option<HICON> {
    unsafe { crate::ui::app_icon(true) }
}
fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid
}

pub fn add(hwnd: HWND) -> bool {
    let Some(icon) = load_tray_icon() else {
        return false;
    };
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    nid.hIcon = icon;
    let tip = util::wide_no_nul("CloudreveLite 分享工具");
    for (i, c) in tip.iter().take(127).enumerate() {
        nid.szTip[i] = *c;
    }
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            return false;
        }
        // Stay on version 0 semantics: classic mouse messages in lParam.
        // Version 4 coordinate packing is easy to misread; GetCursorPos at
        // menu time is simpler and is what proven tray implementations use.
    }
    // The icon handle is intentionally leaked: the tray keeps using it for
    // the whole process lifetime.
    true
}

pub fn remove(hwnd: HWND) {
    let nid = base_nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

pub fn set_tooltip(hwnd: HWND, text: &str) {
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_TIP;
    let tip = util::wide_no_nul(text);
    for (i, c) in tip.iter().take(127).enumerate() {
        nid.szTip[i] = *c;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn balloon(hwnd: HWND, title: &str, text: &str, error: bool) {
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
    let t = util::wide_no_nul(title);
    for (i, c) in t.iter().take(63).enumerate() {
        nid.szInfoTitle[i] = *c;
    }
    let b = util::wide_no_nul(text);
    for (i, c) in b.iter().take(255).enumerate() {
        nid.szInfo[i] = *c;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Show the tray context menu at the cursor and return the chosen command id.
pub fn show_menu(hwnd: HWND, x: i32, y: i32) -> usize {
    unsafe {
        let menu: HMENU = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return 0,
        };
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_SETTINGS,
            windows::core::PCWSTR(util::wide("设置…").as_ptr()),
        );
        let flags = MF_STRING
            | if crate::config::autostart_enabled() {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
        let _ = AppendMenuW(
            menu,
            flags,
            IDM_AUTOSTART,
            windows::core::PCWSTR(util::wide("开机自启").as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_FILES,
            windows::core::PCWSTR(util::wide("文件管理").as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_QUIT,
            windows::core::PCWSTR(util::wide("退出").as_ptr()),
        );

        let _ = SetForegroundWindow(hwnd);
        let tpm = TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD;
        let cmd = TrackPopupMenuEx(menu, tpm.0, x, y, hwnd, None);
        crate::util::log(&format!(
            "menu shown at ({x},{y}), TrackPopupMenuEx returned {}",
            cmd.0
        ));
        // Classic tray-menu fix (KB135788): let the menu dismiss cleanly.
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(hwnd),
            windows::Win32::UI::WindowsAndMessaging::WM_NULL,
            WPARAM(0),
            LPARAM(0),
        );
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}
