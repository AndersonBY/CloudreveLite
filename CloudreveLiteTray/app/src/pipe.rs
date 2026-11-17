use crate::util;
use std::sync::mpsc::Sender;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE, ERROR_PIPE_BUSY, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, NAMED_PIPE_MODE,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

pub const PIPE_NAME: &str = r"\\.\pipe\cloudreve-lite-tray";

const PIPE_ACCESS_INBOUND: u32 = 0x00000001;
const PIPE_TYPE_BYTE: u32 = 0x00000000;
const PIPE_READMODE_BYTE: u32 = 0x00000000;
const PIPE_WAIT: u32 = 0x00000000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const GENERIC_WRITE: u32 = 0x40000000;

/// Server thread: accepts one connection at a time, parses the JSON message
/// {"action":"upload"|"quit","files":[...]} and dispatches it.
pub fn server(hwnd: windows::Win32::Foundation::HWND, tx: Sender<Vec<String>>) {
    loop {
        let name = util::wide(PIPE_NAME);
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_INBOUND),
                NAMED_PIPE_MODE(PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT),
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                None,
            )
        };
        if pipe.is_invalid() {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            continue;
        }
        if unsafe { ConnectNamedPipe(pipe, None) }.is_err() {
            // client connected between create and connect, or error: still try read
        }
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let mut read = 0u32;
            let ok = unsafe { ReadFile(pipe, Some(&mut chunk), Some(&mut read as *mut u32), None) };
            if !ok.is_ok() {
                let err = unsafe { windows::Win32::Foundation::GetLastError() };
                if err != ERROR_BROKEN_PIPE {
                    util::log(&format!("pipe read error: {err:?}"));
                }
                break;
            }
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..read as usize]);
            if buf.len() > 1 << 20 {
                break;
            }
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
        let text = String::from_utf8_lossy(&buf).trim().to_string();
        if text.is_empty() {
            continue;
        }
        util::log(&format!("pipe recv: {}", &text[..text.len().min(200)]));
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                util::log(&format!("pipe json parse error: {e}"));
                continue;
            }
        };
        let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
        match action {
            "quit" => unsafe {
                util::log("pipe: quit requested");
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            "upload" => {
                let files: Vec<String> = v
                    .get("files")
                    .and_then(|f| f.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                util::log(&format!("pipe: upload request, {} file(s)", files.len()));
                if !files.is_empty() {
                    if !crate::upload::enqueue(&tx, files) {
                        let text =
                            Box::into_raw(Box::new("正在安装更新，请稍后重新上传。".to_string()));
                        unsafe {
                            if PostMessageW(
                                Some(hwnd),
                                crate::upload::WM_APP_UPLOAD_DONE,
                                WPARAM(1),
                                LPARAM(text as isize),
                            )
                            .is_err()
                            {
                                drop(Box::from_raw(text));
                            }
                        }
                    }
                }
            }
            _ => {
                util::log(&format!("pipe: unknown action {action:?}"));
            }
        }
    }
}

/// Launcher mode: forward file paths to the running app over the pipe,
/// starting the app first if it is not running yet.
pub fn forward_files(files: &[String]) -> bool {
    let msg = serde_json::json!({ "action": "upload", "files": files }).to_string();
    if send_raw(&msg) {
        return true;
    }
    // Not running: start a resident instance, then retry.
    let exe = std::env::current_exe().unwrap_or_else(|_| "CloudreveTray.exe".into());
    use std::os::windows::process::CommandExt;
    let spawned = std::process::Command::new(&exe)
        .creation_flags(0x00000008 /* DETACHED_PROCESS */)
        .spawn();
    if let Err(e) = spawned {
        eprintln!("failed to start tray app: {e}");
        return false;
    }
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if send_raw(&msg) {
            return true;
        }
    }
    false
}

fn send_raw(msg: &str) -> bool {
    let name = util::wide(PIPE_NAME);
    loop {
        let h = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_WRITE,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        let handle = match h {
            Ok(h) => h,
            Err(_) => {
                let busy = unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_PIPE_BUSY
                    && unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 1000) }.as_bool();
                if busy {
                    continue;
                }
                return false;
            }
        };
        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                handle,
                Some(msg.as_bytes()),
                Some(&mut written as *mut u32),
                None,
            )
            .is_ok()
                && written == msg.len() as u32
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        return ok;
    }
}
