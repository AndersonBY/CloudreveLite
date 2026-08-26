use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, GetStockObject, RoundRect,
    SelectObject, SetBkMode, SetTextColor, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    DEFAULT_QUALITY, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBRUSH, HFONT, NULL_BRUSH,
    OUT_DEFAULT_PRECIS, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    SetWindowTheme, DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowTextW, LoadImageW, SendMessageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR,
    SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_GETFONT, WM_SETFONT,
};

pub const BG: COLORREF = rgb(248, 249, 251);
pub const TEXT: COLORREF = rgb(24, 32, 45);
pub const MUTED: COLORREF = rgb(91, 103, 122);
pub const ACCENT: COLORREF = rgb(37, 99, 235);
pub const SUCCESS: COLORREF = rgb(22, 128, 85);
pub const ERROR: COLORREF = rgb(197, 48, 48);

static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
static DPI: AtomicUsize = AtomicUsize::new(96);

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | ((g as u32) << 8) | ((b as u32) << 16))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn init_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        set_dpi(GetDpiForSystem());
    }
}

pub fn set_dpi(dpi: u32) {
    DPI.store(dpi.max(96) as usize, Ordering::Relaxed);
}

pub fn scale(value: i32) -> i32 {
    let dpi = DPI.load(Ordering::Relaxed) as i32;
    if value < 0 {
        -((-value * dpi + 48) / 96)
    } else {
        (value * dpi + 48) / 96
    }
}

pub unsafe fn app_icon(small: bool) -> Option<HICON> {
    let module = GetModuleHandleW(None).ok()?;
    let (width, height) = if small {
        (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON))
    } else {
        (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON))
    };
    let icon = LoadImageW(
        Some(windows::Win32::Foundation::HINSTANCE(module.0)),
        PCWSTR(101 as *const u16),
        IMAGE_ICON,
        width,
        height,
        LR_DEFAULTCOLOR,
    )
    .ok()?;
    Some(HICON(icon.0 as *mut _))
}

unsafe fn cached_brush(slot: &AtomicIsize, color: COLORREF) -> HBRUSH {
    let existing = slot.load(Ordering::Relaxed);
    if existing != 0 {
        return HBRUSH(existing as *mut _);
    }
    let created = CreateSolidBrush(color);
    let _ = slot.compare_exchange(0, created.0 as isize, Ordering::SeqCst, Ordering::Relaxed);
    let chosen = slot.load(Ordering::Relaxed);
    HBRUSH(chosen as *mut _)
}

pub unsafe fn background_brush() -> HBRUSH {
    cached_brush(&BG_BRUSH, BG)
}

pub unsafe fn font(height: i32, weight: i32) -> HFONT {
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

pub unsafe fn apply_font(hwnd: HWND, font: HFONT) {
    if !hwnd.0.is_null() {
        let _ = SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
    }
}

pub unsafe fn theme(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let explorer = wide("Explorer");
    let _ = SetWindowTheme(hwnd, PCWSTR(explorer.as_ptr()), PCWSTR::null());
}

pub unsafe fn apply_light_frame(hwnd: HWND) {
    let caption = BG;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_CAPTION_COLOR,
        &caption as *const _ as *const _,
        std::mem::size_of::<COLORREF>() as u32,
    );
}

pub unsafe fn draw_button(item: &DRAWITEMSTRUCT, primary: bool) {
    draw_button_tone(item, if primary { 0 } else { 1 });
}

pub unsafe fn draw_danger_button(item: &DRAWITEMSTRUCT) {
    draw_button_tone(item, 2);
}

unsafe fn draw_button_tone(item: &DRAWITEMSTRUCT, tone: u8) {
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
    let (fill, border, text) = if disabled {
        (rgb(229, 233, 239), rgb(213, 219, 227), rgb(144, 153, 168))
    } else if tone == 2 {
        if selected {
            (rgb(185, 28, 28), rgb(185, 28, 28), rgb(255, 255, 255))
        } else {
            (rgb(220, 38, 38), rgb(220, 38, 38), rgb(255, 255, 255))
        }
    } else if tone == 0 {
        if selected {
            (rgb(29, 78, 216), rgb(29, 78, 216), rgb(255, 255, 255))
        } else {
            (ACCENT, ACCENT, rgb(255, 255, 255))
        }
    } else if selected {
        (rgb(236, 240, 246), rgb(177, 187, 201), TEXT)
    } else {
        (rgb(255, 255, 255), rgb(190, 199, 212), TEXT)
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
            if tone != 1 {
                rgb(255, 255, 255)
            } else {
                ACCENT
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
    let mut text_rect = rect;
    if length > 0 {
        let _ = DrawTextW(
            item.hDC,
            &mut label[..length],
            &mut text_rect,
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
