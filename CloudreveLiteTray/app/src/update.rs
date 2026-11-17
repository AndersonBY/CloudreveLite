use crate::{upload, util};
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const WM_UPDATE_EXIT: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 80;
const RELEASES: &str = "https://api.github.com/repos/AndersonBY/CloudreveLite/releases";
const DOWNLOADS: &str = "https://github.com/AndersonBY/CloudreveLite/releases/download/";
const SETUP: &str = "CloudreveLiteTray-Setup.exe";
const MAX_INSTALLER: u64 = 32 * 1024 * 1024;
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

fn attempt_marker() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("CloudreveLiteTray")
            .join("updates")
            .join("last-attempt"),
    )
}

fn startup_delay() -> Duration {
    attempt_marker()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|age| CHECK_INTERVAL.saturating_sub(age))
        .unwrap_or(Duration::from_secs(15))
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

struct Available {
    version: String,
    url: String,
    size: u64,
    sha256: String,
}

struct Pending {
    path: PathBuf,
    sha256: String,
    manual: bool,
}

struct State {
    busy: bool,
    status: String,
    pending: Option<Pending>,
}

static STATE: Mutex<State> = Mutex::new(State {
    busy: false,
    status: String::new(),
    pending: None,
});
static NEXT_CHECK: OnceLock<Mutex<Instant>> = OnceLock::new();

pub fn status() -> (String, bool) {
    let state = STATE.lock().unwrap();
    (state.status.clone(), state.busy)
}

fn set_status(text: String) {
    STATE.lock().unwrap().status = text;
}

fn version(text: &str) -> Option<[u64; 3]> {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ])
}

fn tray_version(release: &Release) -> Option<[u64; 3]> {
    if release.draft || release.prerelease {
        return None;
    }
    version(release.tag_name.strip_prefix("tray-v")?)
}

fn select_release(releases: &[Release], current: [u64; 3]) -> Result<Option<Available>, String> {
    let Some((number, release)) = releases
        .iter()
        .filter_map(|r| tray_version(r).map(|v| (v, r)))
        .max_by_key(|(v, _)| *v)
    else {
        return Ok(None);
    };
    if number <= current {
        return Ok(None);
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == SETUP)
        .ok_or("新版本缺少安装包，请稍后重试。")?;
    let expected = format!("{DOWNLOADS}{}/{SETUP}", release.tag_name);
    if asset.browser_download_url != expected || asset.size == 0 || asset.size > MAX_INSTALLER {
        return Err("更新安装包地址或大小无效。".into());
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("新版本缺少有效的 SHA-256 校验值。")?;
    Ok(Some(Available {
        version: release.tag_name.trim_start_matches("tray-v").into(),
        url: expected,
        size: asset.size,
        sha256: sha256.to_ascii_lowercase(),
    }))
}

fn latest(agent: &ureq::Agent) -> Result<Option<Available>, String> {
    let current = version(VERSION).ok_or("当前版本号无效。")?;
    for page in 1..=10 {
        let mut response = agent
            .get(format!("{RELEASES}?per_page=100&page={page}"))
            .header(
                "User-Agent",
                concat!("CloudreveLiteTray/", env!("CARGO_PKG_VERSION")),
            )
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|_| "无法连接 GitHub 更新服务，请检查网络后重试。")?;
        let mut data = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map_err(|_| "读取版本信息失败。")?;
        if data.len() > 4 * 1024 * 1024 {
            return Err("版本信息过大。".into());
        }
        let releases: Vec<Release> =
            serde_json::from_slice(&data).map_err(|_| "版本信息格式无效。")?;
        if releases.iter().any(|r| tray_version(r).is_some()) {
            return select_release(&releases, current);
        }
        if releases.len() < 100 {
            break;
        }
    }
    Err("未找到托盘工具的正式版本。".into())
}

fn download(agent: &ureq::Agent, available: &Available) -> Result<PathBuf, String> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("无法定位更新目录。")?
        .join("CloudreveLiteTray")
        .join("updates")
        .join(format!(
            "{}-{}",
            available.version,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
    std::fs::create_dir_all(&dir).map_err(|_| "无法创建更新目录。")?;
    let path = dir.join(SETUP);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| "无法保存更新安装包。")?;
    let result = (|| {
        let mut response = agent
            .get(&available.url)
            .call()
            .map_err(|_| "下载安装包失败。")?;
        let mut reader = response.body_mut().as_reader().take(available.size + 1);
        let mut buffer = [0u8; 64 * 1024];
        let mut received = 0u64;
        loop {
            let n = reader.read(&mut buffer).map_err(|_| "下载中断，请重试。")?;
            if n == 0 {
                break;
            }
            received += n as u64;
            if received > available.size {
                return Err("安装包大小不匹配。".to_string());
            }
            file.write_all(&buffer[..n])
                .map_err(|_| "写入安装包失败。")?;
            set_status(format!(
                "正在下载 v{} · {}%",
                available.version,
                received * 100 / available.size
            ));
        }
        if received != available.size {
            return Err("安装包下载不完整。".into());
        }
        file.sync_all().map_err(|_| "保存安装包失败。")?;
        Ok(())
    })();
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

fn verify_installer(path: &Path, sha256: &str) -> Result<(), String> {
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("无法定位 Windows 校验工具。")?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    // Arguments stay out of script text; pin the signer to this running binary.
    let script = r#"$ErrorActionPreference = 'Stop'
if ((Get-FileHash -LiteralPath $env:CR_UPDATE_SETUP -Algorithm SHA256).Hash -ne $env:CR_UPDATE_HASH) { exit 10 }
$installed = Get-AuthenticodeSignature -LiteralPath $env:CR_UPDATE_CURRENT
$downloaded = Get-AuthenticodeSignature -LiteralPath $env:CR_UPDATE_SETUP
if ($installed.Status -ne 'Valid' -or $downloaded.Status -ne 'Valid') { exit 11 }
if ($installed.SignerCertificate.Thumbprint -ne $downloaded.SignerCertificate.Thumbprint) { exit 12 }
exit 0"#;
    let mut child = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("CR_UPDATE_SETUP", path)
        .env("CR_UPDATE_HASH", sha256)
        .env(
            "CR_UPDATE_CURRENT",
            std::env::current_exe().map_err(|_| "无法定位当前程序。")?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| "无法启动安装包校验。")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match child.try_wait().map_err(|_| "读取校验结果失败。")? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(match status.code() {
                    Some(10) => "安装包 SHA-256 不匹配，已停止更新。",
                    Some(12) => "安装包签名者与当前程序不同，已停止更新。",
                    _ => "安装包签名验证失败，已停止更新。",
                }
                .into())
            }
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("安装包校验超时，请稍后重试。".into());
            }
        }
    }
}

pub fn check(manual: bool) {
    {
        let mut state = STATE.lock().unwrap();
        if state.busy {
            return;
        }
        if let Some(pending) = state.pending.as_mut() {
            if manual {
                pending.manual = true;
            }
            return;
        }
        state.busy = true;
        state.status = "正在检查新版本…".into();
    }
    std::thread::spawn(move || {
        let result = (|| {
            let agent = ureq::Agent::config_builder()
                .tls_config(crate::api::tls_config())
                .https_only(true)
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_global(Some(Duration::from_secs(120)))
                .build()
                .new_agent();
            let Some(available) = latest(&agent)? else {
                return Ok(None);
            };
            let path = download(&agent, &available)?;
            set_status(format!("正在校验 v{} 安装包…", available.version));
            if let Err(e) = verify_installer(&path, &available.sha256) {
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
            Ok(Some((
                available.version,
                Pending {
                    path,
                    sha256: available.sha256,
                    manual,
                },
            )))
        })();
        let mut state = STATE.lock().unwrap();
        state.busy = false;
        match result {
            Ok(Some((version, pending))) => {
                state.status = format!("v{version} 已下载，等待上传结束后自动安装。 ");
                state.pending = Some(pending);
                util::log("update: verified installer ready");
            }
            Ok(None) => state.status = format!("当前版本 v{VERSION}，无可用更新。"),
            Err(error) => {
                state.status = error;
                util::log("update: check or download failed");
            }
        }
    });
}

pub fn tick(hwnd: HWND) {
    let enabled = crate::CONFIG.read().unwrap().auto_update;
    let mut next = NEXT_CHECK
        .get_or_init(|| Mutex::new(Instant::now() + startup_delay()))
        .lock()
        .unwrap();
    if enabled && Instant::now() >= *next {
        *next = Instant::now() + CHECK_INTERVAL;
        check(false);
    }
    drop(next);
    let pending = {
        let mut state = STATE.lock().unwrap();
        let Some(pending) = state.pending.as_ref() else {
            return;
        };
        if (!pending.manual && !enabled) || !upload::begin_update() {
            return;
        }
        state.busy = true;
        state.status = "正在安装更新，完成后会自动重新启动。".into();
        state.pending.take().unwrap()
    };
    let hwnd = hwnd.0 as usize;
    std::thread::spawn(move || {
        let result = verify_installer(&pending.path, &pending.sha256).and_then(|_| {
            // Survives a failed installation and the old app's restart.
            let marker = attempt_marker().ok_or("无法记录更新状态。")?;
            std::fs::write(marker, VERSION).map_err(|_| "无法记录更新状态。")?;
            Command::new(&pending.path)
                .args(["--update", &std::process::id().to_string()])
                .creation_flags(0x08000000)
                .spawn()
                .map(|_| ())
                .map_err(|_| "无法启动安装程序，请重新检查更新。".into())
        });
        match result {
            Ok(()) => unsafe {
                let _ = PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    WM_UPDATE_EXIT,
                    WPARAM(0),
                    LPARAM(0),
                );
            },
            Err(error) => {
                upload::abort_update();
                let mut state = STATE.lock().unwrap();
                state.busy = false;
                state.status = error;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        serde_json::from_value(serde_json::json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "assets": [{ "name": SETUP, "size": 123,
                "browser_download_url": format!("{DOWNLOADS}{tag}/{SETUP}"),
                "digest": format!("sha256:{}", "a".repeat(64)) }]
        }))
        .unwrap()
    }

    #[test]
    fn selects_only_newer_stable_tray_versions() {
        let mut beta = release("tray-v9.0.0");
        beta.prerelease = true;
        let mut draft = release("tray-v8.0.0");
        draft.draft = true;
        let releases = [
            release("lite-v40.0.0"),
            beta,
            draft,
            release("tray-v0.3.9"),
            release("tray-v0.3.10"),
            release("tray-v1.0.0-beta"),
        ];
        let update = select_release(&releases, [0, 3, 2]).unwrap().unwrap();
        assert_eq!(update.version, "0.3.10");
        assert!(select_release(&releases, [0, 3, 10]).unwrap().is_none());
        assert!(select_release(&releases, [1, 0, 0]).unwrap().is_none());
        for text in ["0.3", "0.3.2-beta", "+0.3.2", "0.3.2.0", "0..2"] {
            assert!(version(text).is_none());
        }
    }

    #[test]
    fn rejects_untrusted_or_incomplete_installer_metadata() {
        let mut r = release("tray-v0.4.0");
        r.assets[0].browser_download_url = "https://example.com/setup.exe".into();
        assert!(select_release(&[r], [0, 3, 2]).is_err());
        let mut r = release("tray-v0.4.0");
        r.assets[0].digest = None;
        assert!(select_release(&[r], [0, 3, 2]).is_err());
        let mut r = release("tray-v0.4.0");
        r.assets[0].size = MAX_INSTALLER + 1;
        assert!(select_release(&[r], [0, 3, 2]).is_err());
        let mut r = release("tray-v0.4.0");
        r.assets.clear();
        assert!(select_release(&[r], [0, 3, 2]).is_err());
    }
}
