#![windows_subsystem = "windows"]

//! Single-file installer for the CloudreveLite tray app.
//!
//! Install (no arguments): extracts the embedded payload to
//! %LOCALAPPDATA%\CloudreveLiteTray, registers the classic context-menu verb
//! (HKCU), imports the signing cert to the user stores, registers the sparse
//! MSIX for the Windows 11 modern menu, enables autostart and launches the
//! app. Everything is per-user; no elevation required.
//!
//! --uninstall: two-stage. The first stage copies itself to %TEMP% and
//! relaunches from there so the copy in the install directory can be deleted.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

mod ui;

const PIPE_NAME: &str = r"\\.\pipe\cloudreve-lite-tray";
const CLSID: &str = "{AA8A460B-1449-4FA6-B339-B01B527F7FF5}";
const APP_DIR_NAME: &str = "CloudreveLiteTray";
const APP_EXE: &str = "CloudreveTray.exe";
const APP_DLL: &str = "cloudreve_shell.dll";
const MSIX_NAME: &str = "CloudreveShell.msix";
const CER_NAME: &str = "CloudreveLiteTray.cer";
const SETUP_NAME: &str = "CloudreveLiteTray-Setup.exe";
const CERT_SUBJECT: &str = "CN=CloudreveLiteTray";
const PACKAGE_ID: &str = "CloudreveLiteShare";
const GENERIC_WRITE: u32 = 0x40000000;

static UPDATE_LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn init_update_log() {
    let Some(base) = std::env::var_os("APPDATA") else {
        return;
    };
    let dir = PathBuf::from(base).join(APP_DIR_NAME);
    if std::fs::create_dir_all(&dir).is_ok() {
        *UPDATE_LOG.lock().unwrap() = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(dir.join("update.log"))
            .ok();
    }
}

static PAYLOAD_EXE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/payload/CloudreveTray.exe"
));
static PAYLOAD_DLL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/payload/cloudreve_shell.dll"
));
static PAYLOAD_MSIX: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/payload/CloudreveShell.msix"
));
static PAYLOAD_CER: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/payload/CloudreveLiteTray.cer"
));

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn step(msg: &str) {
    ui::step(msg);
    if let Some(file) = UPDATE_LOG.lock().unwrap().as_mut() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(file, "[{now}] {msg}");
    }
}

fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    base.join(APP_DIR_NAME)
}

// ------------------------------------------------------------ registry utils ---

unsafe fn reg_set(hkey: HKEY, name: &str, value: &str) -> bool {
    let wval = wide(value);
    let bytes = std::slice::from_raw_parts(wval.as_ptr() as *const u8, wval.len() * 2);
    let r = RegSetValueExW(hkey, PCWSTR(wide(name).as_ptr()), None, REG_SZ, Some(bytes));
    r == windows::Win32::Foundation::WIN32_ERROR(0)
}

unsafe fn create_key(path: &str) -> Option<HKEY> {
    let mut hkey: HKEY = Default::default();
    let r = RegCreateKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(wide(path).as_ptr()),
        None,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut hkey,
        None,
    );
    if r == windows::Win32::Foundation::WIN32_ERROR(0) {
        Some(hkey)
    } else {
        None
    }
}

unsafe fn delete_tree(path: &str) {
    let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(path).as_ptr()));
}

unsafe fn delete_value(path: &str, value: &str) {
    if let Some(hkey) = create_key(path) {
        let _ = RegDeleteValueW(hkey, PCWSTR(wide(value).as_ptr()));
        let _ = RegCloseKey(hkey);
    }
}

// -------------------------------------------------------------- pipe helpers ---

fn send_pipe_json(json: &str) -> bool {
    let name = wide(PIPE_NAME);
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
        Err(_) => return false,
    };
    let mut written = 0u32;
    let ok = unsafe {
        WriteFile(
            handle,
            Some(json.as_bytes()),
            Some(&mut written as *mut u32),
            None,
        )
        .is_ok()
            && written == json.len() as u32
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    ok
}

fn quit_running_app() {
    if send_pipe_json(r#"{"action":"quit"}"#) {
        step("已请求托盘进程退出…");
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}

fn run_powershell(args: &str, hidden: bool) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", args]);
    if hidden {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    match cmd.output() {
        Ok(out) => Some(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Some(format!("spawn failed: {e}")),
    }
}

/// Run PowerShell and return its exit code.
fn run_powershell_code(args: &str) -> Option<i32> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", args]);
    cmd.creation_flags(0x08000000);
    cmd.status().ok().map(|s| s.code().unwrap_or(-1))
}

fn utf16le_b64(s: &str) -> String {
    // base64 of UTF-16LE bytes, for PowerShell -EncodedCommand
    let mut bytes = Vec::with_capacity(s.len() * 2);
    for u in s.encode_utf16() {
        bytes.push(u as u8);
        bytes.push((u >> 8) as u8);
    }
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Elevate via UAC and run a PowerShell script (hidden window) as admin.
/// Returns the elevated script's exit code, or None when launch failed
/// (UAC declined). Uses -EncodedCommand to avoid all quoting issues.
fn elevate_ps(script: &str) -> Option<i32> {
    let enc = utf16le_b64(script);
    let launcher = format!(
        "$p = Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -PassThru \
         -ArgumentList '-NoProfile -ExecutionPolicy Bypass -EncodedCommand {enc}'; exit $p.ExitCode"
    );
    run_powershell_code(&launcher)
}

/// True when our signing cert is already in the machine Root store.
fn machine_cert_present() -> bool {
    run_powershell(
        &format!(
            "$c = Get-ChildItem Cert:\\LocalMachine\\Root -ErrorAction SilentlyContinue | Where-Object {{ $_.Subject -eq '{CERT_SUBJECT}' }}; if ($c) {{ 'YES' }}"
        ),
        true,
    )
    .as_deref()
    .map(|s| s.contains("YES"))
    .unwrap_or(false)
}

/// Best-effort cleanup of user-store copies (no admin needed).
fn remove_cert_user() {
    let _ = run_powershell(
        &format!(
            "Get-ChildItem Cert:\\CurrentUser\\Root, Cert:\\CurrentUser\\TrustedPublisher -ErrorAction SilentlyContinue | \
             Where-Object {{ $_.Subject -eq '{CERT_SUBJECT}' }} | Remove-Item -ErrorAction SilentlyContinue"
        ),
        true,
    );
}

fn refresh_shell() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.display()
        .to_string()
        .eq_ignore_ascii_case(&b.display().to_string())
}

// ------------------------------------------------------------------- install ---

fn write_payload(dir: &Path, name: &str, data: &[u8]) -> Result<(), String> {
    let target = dir.join(name);
    if std::fs::write(&target, data).is_err() {
        // Explorer can retain the old DLL until its COM host exits.
        let retired = dir.join("backups").join(format!(
            "in-use-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&retired).map_err(|_| "无法保留被占用的旧程序文件。")?;
        std::fs::rename(&target, retired.join(name))
            .map_err(|_| format!("无法替换 {name}，请关闭占用程序后重试。"))?;
        std::fs::write(&target, data).map_err(|_| format!("无法写入 {name}。"))?;
    }
    Ok(())
}

fn install(update: bool) -> Result<(bool, bool), String> {
    ui::note("正在准备安装文件。整个程序安装到当前用户，不会修改系统级应用目录。");

    let dir = install_dir();
    if !update {
        quit_running_app();
    }

    step(&format!("创建目录 {}", dir.display()));
    std::fs::create_dir_all(&dir).map_err(|_| "无法创建安装目录。")?;

    step("释放文件…");
    write_payload(&dir, APP_EXE, PAYLOAD_EXE)?;
    write_payload(&dir, APP_DLL, PAYLOAD_DLL)?;
    write_payload(&dir, MSIX_NAME, PAYLOAD_MSIX)?;
    write_payload(&dir, CER_NAME, PAYLOAD_CER)?;

    // Keep a copy of the setup itself so "Add/Remove Programs" can find it.
    if let Ok(self_path) = std::env::current_exe() {
        if !same_path(&self_path, &dir.join(SETUP_NAME)) {
            std::fs::copy(&self_path, dir.join(SETUP_NAME))
                .map_err(|_| "无法更新安装程序副本。")?;
        }
    }

    let exe_path = dir.join(APP_EXE);
    let dll_path = dir.join(APP_DLL);
    let msix_path = dir.join(MSIX_NAME);
    let cer_path = dir.join(CER_NAME);

    step("写入注册表（经典右键菜单 / 自启 / 卸载信息）…");
    unsafe {
        if let Some(k) = create_key(r"SOFTWARE\CloudreveLiteTray") {
            reg_set(k, "InstallPath", &dir.display().to_string());
            let _ = RegCloseKey(k);
        }
        if let Some(k) = create_key(&format!(r"SOFTWARE\Classes\CLSID\{CLSID}\InProcServer32")) {
            reg_set(k, "", &dll_path.display().to_string());
            reg_set(k, "ThreadingModel", "Apartment");
            let _ = RegCloseKey(k);
        }
        let verb = r"SOFTWARE\Classes\*\shell\CloudreveLiteShare";
        if let Some(k) = create_key(verb) {
            reg_set(k, "MUIVerb", "上传到 CloudreveLite 并分享");
            reg_set(k, "ExplorerCommandHandler", CLSID);
            reg_set(k, "Icon", &format!("{},0", exe_path.display()));
            let _ = RegCloseKey(k);
        }
        if !update {
            if let Some(k) = create_key(r"Software\Microsoft\Windows\CurrentVersion\Run") {
                reg_set(
                    k,
                    "CloudreveLiteTray",
                    &format!("\"{}\"", exe_path.display()),
                );
                let _ = RegCloseKey(k);
            }
        }
        if let Some(k) =
            create_key(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\CloudreveLiteTray")
        {
            reg_set(k, "DisplayName", "CloudreveLite 托盘分享工具");
            reg_set(k, "DisplayVersion", env!("CARGO_PKG_VERSION"));
            reg_set(k, "Publisher", "CloudreveLiteTray");
            reg_set(k, "DisplayIcon", &format!("{},0", exe_path.display()));
            reg_set(
                k,
                "UninstallString",
                &format!("\"{}\" --uninstall", dir.join(SETUP_NAME).display()),
            );
            reg_set(k, "NoModify", "1");
            reg_set(k, "NoRepair", "1");
            let _ = RegCloseKey(k);
        }
    }

    step("准备签名证书（Windows 11 现代菜单需要）…");
    if machine_cert_present() {
        println!("  证书已在机器根存储中。");
        remove_cert_user();
    } else {
        ui::note("Windows 11 现代右键菜单需要一次系统确认。请在接下来的 UAC 窗口中选择“是”。");
        let import_script = format!(
            "try {{ Import-Certificate -FilePath '{}' -CertStoreLocation Cert:\\LocalMachine\\Root | Out-Null; \
             Import-Certificate -FilePath '{}' -CertStoreLocation Cert:\\LocalMachine\\TrustedPublisher | Out-Null; exit 0 }} \
             catch {{ exit 1 }}",
            cer_path.display(),
            cer_path.display()
        );
        match elevate_ps(&import_script) {
            Some(0) => {
                println!("  证书已导入机器根存储。");
                remove_cert_user();
            }
            code => {
                ui::note(&format!(
                    "系统确认未完成（{code:?}），将继续安装。右键“显示更多选项”中的经典菜单仍可使用。"
                ));
            }
        }
    }
    let cert_ok = !update && machine_cert_present();

    step("注册 Windows 11 现代右键菜单（稀疏 MSIX）…");
    let mut modern_menu_ok = false;
    if cert_ok {
        let _ = run_powershell(
            &format!(
                "Get-AppxPackage -Name {PACKAGE_ID} -ErrorAction SilentlyContinue | Remove-AppxPackage -ErrorAction SilentlyContinue"
            ),
            true,
        );
        let msix_out = run_powershell(
            &format!(
                "try {{ Add-AppxPackage -Path '{}' -ExternalLocation '{}' -ErrorAction Stop; 'OK' }} catch {{ $_.Exception.Message }}",
                msix_path.display(),
                dir.display()
            ),
            true,
        );
        let msix_ok = msix_out
            .as_deref()
            .map(|s| s.contains("OK"))
            .unwrap_or(false);
        if msix_ok {
            modern_menu_ok = true;
        } else {
            ui::note(&format!(
                "Windows 11 一级右键菜单未能注册（{msix_out:?}）。经典右键菜单仍可使用。"
            ));
        }
    }

    refresh_shell();

    step("创建开始菜单快捷方式…");
    let _ = run_powershell(
        &format!(
            "$lnk = Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs\\CloudreveLiteTray.lnk'; \
             $ws = New-Object -ComObject WScript.Shell; \
             $s = $ws.CreateShortcut($lnk); \
             $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{},0'; \
             $s.Description = 'CloudreveLite 托盘分享工具'; \
             $s.Save()",
            exe_path.display(),
            dir.display(),
            exe_path.display()
        ),
        true,
    );

    step("启动托盘程序…");
    use std::os::windows::process::CommandExt;
    let launched = match Command::new(&exe_path).creation_flags(0x00000008).spawn() {
        Ok(mut child) if update => {
            std::thread::sleep(std::time::Duration::from_secs(1));
            matches!(child.try_wait(), Ok(None))
        }
        Ok(_) => true,
        Err(_) => false,
    };
    if !launched {
        println!("  警告: 启动失败，请手动运行 {}", exe_path.display());
    }

    Ok((modern_menu_ok, launched))
}

fn update_parent(args: &[String]) -> Result<u32, String> {
    if args.len() != 2 {
        return Err("更新参数无效。".into());
    }
    args[1]
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0 && *pid != std::process::id())
        .ok_or_else(|| "更新进程编号无效。".into())
}

fn wait_for_parent(pid: u32) -> Result<(), String> {
    use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    unsafe {
        let process = match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(process) => process,
            Err(error)
                if error.code()
                    == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
            {
                return Ok(())
            }
            Err(_) => return Err("无法等待旧版程序退出。".into()),
        };
        let result = WaitForSingleObject(process, 30_000);
        let _ = CloseHandle(process);
        if result != WAIT_OBJECT_0 {
            return Err("旧版程序尚未退出，未执行更新。".into());
        }
    }
    Ok(())
}

const UPDATE_FILES: [&str; 5] = [APP_EXE, APP_DLL, MSIX_NAME, CER_NAME, SETUP_NAME];

fn backup_update(dir: &Path) -> Result<PathBuf, String> {
    let backup = dir.join("backups").join(format!(
        "update-{}-{}",
        env!("CARGO_PKG_VERSION"),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&backup).map_err(|_| "无法创建更新备份目录。")?;
    for name in UPDATE_FILES {
        std::fs::copy(dir.join(name), backup.join(name))
            .map_err(|_| format!("无法备份 {name}，未执行更新。"))?;
    }
    Ok(backup)
}

fn restore_update(dir: &Path, backup: &Path) -> Result<(), String> {
    for name in UPDATE_FILES {
        let data =
            std::fs::read(backup.join(name)).map_err(|_| format!("无法读取 {name} 的备份。"))?;
        write_payload(dir, name, &data)?;
    }
    Ok(())
}

fn install_update(pid: u32) -> Result<(), String> {
    step("等待旧进程退出");
    wait_for_parent(pid)?;
    let dir = install_dir();
    let result = (|| {
        step("备份现有程序文件");
        let backup = backup_update(&dir)?;
        match install(true) {
            Ok((_, true)) => Ok(()),
            result => {
                let error = match result {
                    Err(e) => e,
                    _ => "新版程序未能启动。".into(),
                };
                restore_update(&dir, &backup).map_err(|restore| {
                    format!("{error} 恢复失败：{restore} 备份目录：{}", backup.display())
                })?;
                Err(format!("{error} 已恢复原程序文件。"))
            }
        }
    })();
    if result.is_err() {
        use std::os::windows::process::CommandExt;
        let _ = Command::new(dir.join(APP_EXE))
            .creation_flags(0x00000008)
            .spawn();
    }
    result
}

// ----------------------------------------------------------------- uninstall ---

fn uninstall_stage1() -> Result<(), String> {
    let dir = install_dir();
    // Copy self to TEMP and relaunch from there so the copy inside the
    // install directory can be deleted.
    let temp = std::env::temp_dir().join(format!(
        "CloudreveLiteTray-uninstall-{}.exe",
        std::process::id()
    ));
    std::fs::copy(
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from(SETUP_NAME)),
        &temp,
    )
    .map_err(|e| format!("无法准备卸载程序：{e}"))?;
    Command::new(&temp)
        .arg("--uninstall-final")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("无法启动卸载程序：{e}"))?;
    Ok(())
}

fn uninstall_final(dir: &Path) {
    println!("CloudreveLiteTray 卸载程序\n");

    if send_pipe_json(r#"{"action":"quit"}"#) {
        step("已请求托盘进程退出…");
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }

    step("移除稀疏 MSIX 包…");
    let _ = run_powershell(
        &format!(
            "Get-AppxPackage -Name {PACKAGE_ID} -ErrorAction SilentlyContinue | Remove-AppxPackage -ErrorAction SilentlyContinue"
        ),
        true,
    );

    step("清理注册表…");
    unsafe {
        delete_tree(&format!(r"SOFTWARE\Classes\CLSID\{CLSID}"));
        delete_tree(r"SOFTWARE\Classes\*\shell\CloudreveLiteShare");
        delete_tree(r"SOFTWARE\CloudreveLiteTray");
        delete_tree(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\CloudreveLiteTray");
        delete_value(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            "CloudreveLiteTray",
        );
    }

    step("移除证书…");
    remove_cert_user();
    if machine_cert_present() {
        println!("  将弹出一次 UAC 确认框（Windows PowerShell）从机器存储移除证书…");
        let remove_script = format!(
            "Get-ChildItem Cert:\\LocalMachine\\Root, Cert:\\LocalMachine\\TrustedPublisher -ErrorAction SilentlyContinue | \
             Where-Object {{ $_.Subject -eq '{CERT_SUBJECT}' }} | Remove-Item -ErrorAction SilentlyContinue; exit 0"
        );
        let _ = elevate_ps(&remove_script);
    }

    refresh_shell();

    step("移除快捷方式…");
    let _ = run_powershell(
        "Remove-Item (Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs\\CloudreveLiteTray.lnk') -ErrorAction SilentlyContinue",
        true,
    );

    step("删除程序文件…");
    let mut names: Vec<String> = [APP_EXE, APP_DLL, MSIX_NAME, CER_NAME, SETUP_NAME]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let olds: Vec<String> = names.iter().map(|n| format!("{n}.old")).collect();
    names.extend(olds);
    for name in names {
        let _ = std::fs::remove_file(dir.join(name));
    }
    if dir.exists()
        && std::fs::read_dir(dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(dir);
    }

    // Self-delete the temp copy.
    let self_path = std::env::current_exe().unwrap_or_default();
    if self_path
        .file_name()
        .map(|n| {
            n.to_string_lossy()
                .starts_with("CloudreveLiteTray-uninstall")
        })
        .unwrap_or(false)
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("cmd.exe")
            .args([
                "/C", "timeout", "/T", "2", "/NOBREAK", ">NUL", "&", "del", "/F", "/Q",
            ])
            .arg(&self_path)
            .creation_flags(0x08000000)
            .spawn();
    }
}

fn main() {
    ui::init_dpi();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("--update") => {
            init_update_log();
            let result = update_parent(&args).and_then(install_update);
            if let Err(error) = result {
                step(&format!("自动更新失败：{error}"));
                std::process::exit(1);
            }
            step("自动更新完成");
        }
        Some("--uninstall") => {
            if let Err(e) = uninstall_stage1() {
                ui::show_error(&e);
            }
        }
        Some("--uninstall-final") => {
            let dir = args.get(1).map(PathBuf::from).unwrap_or_else(install_dir);
            ui::run(ui::Mode::Uninstall, move || {
                uninstall_final(&dir);
                ui::finish(
                    true,
                    "程序文件和右键菜单已移除。连接配置、日志与更新备份仍保留在用户目录中。",
                );
            });
        }
        _ => {
            ui::run(ui::Mode::Install, || {
                let (modern_menu, launched) = match install(false) {
                    Ok(result) => result,
                    Err(error) => {
                        ui::finish(false, &error);
                        return;
                    }
                };
                let detail = match (modern_menu, launched) {
                (true, true) => "托盘程序已经启动。首次使用时，请在设置窗口中粘贴网页生成的配对码。",
                (false, true) => "托盘程序已经启动。经典右键菜单可用；Windows 11 一级菜单可稍后重新安装启用。",
                (_, false) => "文件已经安装，但托盘程序未能自动启动。请从开始菜单打开 CloudreveLiteTray。",
            };
                ui::finish(true, detail);
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_arguments_require_a_valid_parent() {
        assert!(update_parent(&["--update".into()]).is_err());
        for value in [
            "0".to_string(),
            "bad".to_string(),
            std::process::id().to_string(),
        ] {
            assert!(update_parent(&["--update".into(), value]).is_err());
        }
        assert_eq!(update_parent(&["--update".into(), "1".into()]).unwrap(), 1);
    }

    #[test]
    fn update_backup_restores_only_program_files() {
        let dir = std::env::temp_dir().join(format!(
            "cloudreve-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        for name in UPDATE_FILES {
            std::fs::write(dir.join(name), b"old").unwrap();
        }
        std::fs::write(dir.join("config.json"), b"keep").unwrap();
        let backup = backup_update(&dir).unwrap();
        for name in UPDATE_FILES {
            write_payload(&dir, name, b"new").unwrap();
        }
        restore_update(&dir, &backup).unwrap();
        for name in UPDATE_FILES {
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), b"old");
            std::fs::remove_file(dir.join(name)).unwrap();
            std::fs::remove_file(backup.join(name)).unwrap();
        }
        assert_eq!(std::fs::read(dir.join("config.json")).unwrap(), b"keep");
        std::fs::remove_file(dir.join("config.json")).unwrap();
        std::fs::remove_dir(&backup).unwrap();
        std::fs::remove_dir(dir.join("backups")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn repeated_updates_can_replace_dlls_still_loaded_by_explorer() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!(
            "cloudreve-locked-update-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join(APP_DLL);
        std::fs::write(&file, b"v1").unwrap();
        let first = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 4)
            .open(&file)
            .unwrap();
        write_payload(&dir, APP_DLL, b"v2").unwrap();
        let second = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 4)
            .open(&file)
            .unwrap();
        write_payload(&dir, APP_DLL, b"v3").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"v3");
        drop((first, second));
        std::fs::remove_file(&file).unwrap();
        let retired = std::fs::read_dir(dir.join("backups"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(retired.len(), 2);
        for entry in retired {
            std::fs::remove_file(entry.path().join(APP_DLL)).unwrap();
            std::fs::remove_dir(entry.path()).unwrap();
        }
        std::fs::remove_dir(dir.join("backups")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
