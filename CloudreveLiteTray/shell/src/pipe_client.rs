use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;

pub const PIPE_NAME: &str = r"\\.\pipe\cloudreve-lite-tray";
const REG_SOFTWARE_KEY: &str = r"SOFTWARE\CloudreveLiteTray";
const APP_EXE: &str = "CloudreveTray.exe";

pub fn send_upload_request(files: &[String]) -> Result<()> {
    let msg = serde_json::json!({
        "action": "upload",
        "files": files
    });
    let msg_bytes = msg.to_string().into_bytes();

    let pipe_name = HSTRING::from(PIPE_NAME);

    let handle = unsafe {
        CreateFileW(
            &pipe_name,
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )?
    };

    unsafe {
        let mut written = 0u32;
        WriteFile(handle, Some(&msg_bytes), Some(&mut written as *mut u32), None)?;
        let _ = CloseHandle(handle);
    }

    Ok(())
}

/// Read the installed app path and return the exe for icon lookup.
pub fn installed_app_icon() -> Option<String> {
    let dir = read_install_dir()?;
    Some(format!(r"{}\{APP_EXE}", dir))
}

fn read_install_dir() -> Option<String> {
    use windows::Win32::System::Registry::*;

    let subkey = HSTRING::from(REG_SOFTWARE_KEY);
    let mut hkey = HKEY::default();
    let result = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, None, KEY_READ, &mut hkey) };
    if result != ERROR_SUCCESS {
        return None;
    }

    let value_name = HSTRING::from("InstallPath");
    let mut buf = [0u16; 512];
    let mut buf_size = (buf.len() * 2) as u32;
    let mut reg_type = REG_VALUE_TYPE::default();
    let result = unsafe {
        RegQueryValueExW(
            hkey,
            &value_name,
            None,
            Some(&mut reg_type),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut buf_size as *mut u32),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }

    if result == ERROR_SUCCESS {
        let len = (buf_size as usize / 2).saturating_sub(1);
        let path = String::from_utf16_lossy(&buf[..len]);
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

/// Launch the tray app when the pipe is not available.
pub fn try_launch_main_process() {
    let Some(dir) = read_install_dir() else {
        return;
    };
    let exe_path = format!(r"{}\{APP_EXE}", dir);
    let _ = std::process::Command::new(&exe_path).spawn();
    std::thread::sleep(std::time::Duration::from_secs(2));
}
