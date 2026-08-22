use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

// ---------------------------------------------------------------- logging ---

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn log_path() -> std::path::PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("CloudreveLiteTray");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("app.log")
}

pub fn log_init() {
    if let Ok(f) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path())
    {
        *LOG.lock().unwrap() = Some(f);
    }
}

pub fn log(msg: &str) {
    let mut guard = LOG.lock().unwrap();
    if let Some(f) = guard.as_mut() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{now}] {msg}");
    }
}

// ------------------------------------------------------------- conversions ---

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_no_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

// -------------------------------------------------------------- clipboard ---

pub fn set_clipboard(text: &str) -> bool {
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = w.len() * 2;
    unsafe {
        for _ in 0..5 {
            if OpenClipboard(None).is_ok() {
                if EmptyClipboard().is_err() {
                    let _ = CloseClipboard();
                    continue;
                }
                let h = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
                    Ok(h) => h,
                    Err(_) => {
                        let _ = CloseClipboard();
                        return false;
                    }
                };
                let dst = GlobalLock(h);
                if dst.is_null() {
                    let _ = CloseClipboard();
                    return false;
                }
                std::ptr::copy_nonoverlapping(w.as_ptr(), dst as *mut u16, w.len());
                let _ = GlobalUnlock(h);
                let ok = SetClipboardData(0x0d /* CF_UNICODETEXT */, Some(HANDLE(h.0))).is_ok();
                let _ = CloseClipboard();
                return ok;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    false
}

// ------------------------------------------------------------- open things ---

pub fn open_url(url: &str) {
    let wurl = wide(url);
    unsafe {
        ShellExecuteW(
            None,
            PCWSTR(wide("open").as_ptr()),
            PCWSTR(wurl.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

// -------------------------------------------------------------- mime type ---

const MIME_TABLE: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("heic", "image/heic"),
    ("avif", "image/avif"),
    ("mp4", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
    ("avi", "video/x-msvideo"),
    ("mp3", "audio/mpeg"),
    ("flac", "audio/flac"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("pdf", "application/pdf"),
    ("txt", "text/plain"),
    ("log", "text/plain"),
    ("md", "text/markdown"),
    ("csv", "text/csv"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("xml", "application/xml"),
    ("json", "application/json"),
    ("zip", "application/zip"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/vnd.rar"),
    ("tar", "application/x-tar"),
    ("gz", "application/gzip"),
    ("doc", "application/msword"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("xls", "application/vnd.ms-excel"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    ("ppt", "application/vnd.ms-powerpoint"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("apk", "application/vnd.android.package-archive"),
    ("exe", "application/vnd.microsoft.portable-executable"),
    ("msi", "application/x-msdownload"),
    ("iso", "application/x-iso9660-image"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
];

/// Guess a MIME type by extension: registry "Content Type" first, then a
/// built-in table, then application/octet-stream.
pub fn guess_mime(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext.is_empty() {
        return "application/octet-stream".into();
    }
    let subkey = format!(".{ext}");
    let mut buf = [0u16; 256];
    let mut cb = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            PCWSTR(wide(&subkey).as_ptr()),
            PCWSTR(wide("Content Type").as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut cb as *mut u32),
        )
    };
    if ok == windows::Win32::Foundation::WIN32_ERROR(0) && cb >= 2 {
        let len = (cb as usize / 2).saturating_sub(1);
        let s = String::from_utf16_lossy(&buf[..len]);
        let s = s.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    for (k, v) in MIME_TABLE {
        if *k == ext {
            return (*v).into();
        }
    }
    "application/octet-stream".into()
}

// -------------------------------------------------------- percent encoding ---

/// Percent-encode a path segment for a cloudreve:// URI. Keeps only RFC 3986
/// unreserved characters and '/'.
pub fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '/') {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encode a query component (also escapes '/').
pub fn percent_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ------------------------------------------------------------------ base64 ---

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut vals = Vec::with_capacity(s.len());
    for c in s.bytes() {
        if c == b'=' || c == b'\r' || c == b'\n' {
            continue;
        }
        let v = B64.iter().position(|&x| x == c)? as u32;
        vals.push(v);
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        let mut n: u32 = 0;
        for (i, v) in chunk.iter().enumerate() {
            n |= v << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}
