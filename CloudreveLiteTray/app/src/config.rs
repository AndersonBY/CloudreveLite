use crate::util;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: String,
    /// DPAPI-encrypted + base64. Empty when not connected yet.
    pub api_key_enc: String,
    pub expire_days: u32,
    pub downloads: u32,
    pub private: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: "https://share.zaowanwu.com".into(),
            api_key_enc: String::new(),
            expire_days: 7,
            downloads: 10,
            private: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ConfigFile {
    server: String,
    api_key_enc: String,
    expire_days: u32,
    downloads: u32,
    private: bool,
}

pub fn config_path() -> PathBuf {
    util::log_path().parent().unwrap().join("config.json")
}

pub fn load() -> Config {
    let Ok(text) = std::fs::read_to_string(config_path()) else {
        return Config::default();
    };
    // Tolerate a UTF-8 BOM written by PowerShell/editor tooling.
    let text = text.trim_start_matches('\u{feff}');
    match serde_json::from_str::<ConfigFile>(text) {
        Ok(f) => Config {
            server: f.server.trim().trim_end_matches('/').to_string(),
            api_key_enc: f.api_key_enc,
            expire_days: f.expire_days.clamp(1, 30),
            downloads: f.downloads.clamp(1, 100),
            private: f.private,
        },
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> bool {
    let f = ConfigFile {
        server: cfg.server.clone(),
        api_key_enc: cfg.api_key_enc.clone(),
        expire_days: cfg.expire_days,
        downloads: cfg.downloads,
        private: cfg.private,
    };
    let Ok(text) = serde_json::to_string_pretty(&f) else {
        return false;
    };
    let path = config_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text.as_bytes())
        .and_then(|_| std::fs::rename(&tmp, &path))
        .is_err()
    {
        std::fs::write(&path, text.as_bytes()).is_ok()
    } else {
        true
    }
}

// -------------------------------------------------------------------- DPAPI ---

fn dp(blob: &mut CRYPT_INTEGER_BLOB, protect: bool) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    let r = if protect {
        unsafe {
            CryptProtectData(
                blob,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
    } else {
        unsafe {
            CryptUnprotectData(
                blob,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
    };
    if r.is_err() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
    }
    Some(bytes)
}

pub fn encrypt_key(key: &str) -> String {
    let mut data = key.as_bytes().to_vec();
    let mut blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_mut_ptr(),
    };
    dp(&mut blob, true)
        .map(|v| util::b64_encode(&v))
        .unwrap_or_default()
}

pub fn decrypt_key(enc: &str) -> String {
    let Some(mut data) = util::b64_decode(enc) else {
        return String::new();
    };
    if data.is_empty() {
        return String::new();
    }
    let mut blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_mut_ptr(),
    };
    dp(&mut blob, false)
        .map(|v| String::from_utf8_lossy(&v).trim().to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- autostart ---

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "CloudreveLiteTray";

pub fn autostart_enabled() -> bool {
    let mut buf = [0u16; 512];
    let mut cb = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(util::wide(RUN_KEY).as_ptr()),
            PCWSTR(util::wide(RUN_NAME).as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut cb as *mut u32),
        )
    };
    ok == windows::Win32::Foundation::WIN32_ERROR(0)
}

pub fn set_autostart(enable: bool) -> bool {
    if !enable {
        unsafe {
            let mut opened: HKEY = Default::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(util::wide(RUN_KEY).as_ptr()),
                None,
                KEY_READ,
                &mut opened,
            ) == ERROR_SUCCESS
            {
                let r = RegDeleteValueW(opened, PCWSTR(util::wide(RUN_NAME).as_ptr()));
                let _ = RegCloseKey(opened);
                return r == ERROR_SUCCESS;
            }
        }
        return true;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let quoted = format!("\"{}\"", exe.display());
    let wval = util::wide(&quoted);
    let mut hkey: HKEY = Default::default();
    unsafe {
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(util::wide(RUN_KEY).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let bytes = std::slice::from_raw_parts(wval.as_ptr() as *const u8, wval.len() * 2);
        let r = RegSetValueExW(
            hkey,
            PCWSTR(util::wide(RUN_NAME).as_ptr()),
            None,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(hkey);
        r == ERROR_SUCCESS
    }
}
