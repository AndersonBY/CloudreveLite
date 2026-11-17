use crate::{config, ui, util};
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{SetBkMode, SetTextColor, HDC, TRANSPARENT};
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    DialogBoxIndirectParamW, EndDialog, GetDlgCtrlID, GetDlgItem, GetDlgItemTextW,
    SendDlgItemMessageW, SetDlgItemTextW, DLGTEMPLATE, WM_CLOSE, WM_COMMAND, WM_CTLCOLORDLG,
    WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_INITDIALOG,
};

const IDC_TITLE: i32 = 1000;
const IDC_SERVER: i32 = 1001;
const IDC_KEY: i32 = 1002;
const IDC_EXPIRE: i32 = 1003;
const IDC_DOWNLOADS: i32 = 1004;
const IDC_PRIVATE: i32 = 1005;
const IDC_TEST: i32 = 1006;
const IDC_HINT: i32 = 1007;
const IDC_SUBTITLE: i32 = 1008;
const IDC_SHOW_KEY: i32 = 1009;
const IDC_SECTION_SHARE: i32 = 1010;
const IDC_STATUS: i32 = 1011;
const IDC_SECTION_UPDATE: i32 = 1012;
const IDC_VERSION: i32 = 1013;
const IDC_AUTO_UPDATE: i32 = 1014;
const IDC_CHECK_UPDATE: i32 = 1015;
const IDC_UPDATE_STATUS: i32 = 1016;
const TIMER_UPDATE: usize = 81;
const IDOK: i32 = 1;
const IDCANCEL: i32 = 2;

const CB_ADDSTRING: u32 = 0x0143;
const CB_SETCURSEL: u32 = 0x014E;
const CB_GETCURSEL: u32 = 0x0147;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const BST_CHECKED: isize = 1;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETPASSWORDCHAR: u32 = 0x00CC;

const STATUS_MUTED: i32 = 0;
const STATUS_INFO: i32 = 1;
const STATUS_SUCCESS: i32 = 2;
const STATUS_ERROR: i32 = 3;

const DAYS: [u32; 5] = [1, 3, 7, 14, 30];

static STATUS_KIND: AtomicI32 = AtomicI32::new(STATUS_MUTED);
static FONT_TITLE: AtomicIsize = AtomicIsize::new(0);
static FONT_BODY: AtomicIsize = AtomicIsize::new(0);
static FONT_STRONG: AtomicIsize = AtomicIsize::new(0);
static WINDOW: AtomicIsize = AtomicIsize::new(0);

struct Tpl {
    buf: Vec<u16>,
    items: u16,
}

impl Tpl {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            items: 0,
        }
    }
    fn dw(&mut self, value: u32) {
        self.buf.push((value & 0xffff) as u16);
        self.buf.push((value >> 16) as u16);
    }
    fn w(&mut self, value: u16) {
        self.buf.push(value);
    }
    fn xy(&mut self, value: i16) {
        self.buf.push(value as u16);
    }
    fn str(&mut self, value: &str) {
        self.buf.extend(value.encode_utf16());
        self.buf.push(0);
    }
    fn align4(&mut self) {
        if self.buf.len() % 2 == 1 {
            self.buf.push(0);
        }
    }
    fn item(
        &mut self,
        style: u32,
        x: i16,
        y: i16,
        width: i16,
        height: i16,
        id: u16,
        class: u16,
        title: &str,
    ) {
        self.align4();
        self.dw(style);
        self.dw(0);
        self.xy(x);
        self.xy(y);
        self.xy(width);
        self.xy(height);
        self.w(id);
        self.w(0xffff);
        self.w(class);
        self.str(title);
        self.w(0);
        self.items += 1;
    }
}

const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_NUMBER: u32 = 0x2000;
const ES_PASSWORD: u32 = 0x0020;
const SS_ETCHEDHORZ: u32 = 0x0010;
const WS_CHILD: u32 = 0x40000000;
const WS_VISIBLE: u32 = 0x10000000;
const WS_TABSTOP: u32 = 0x00010000;
const WS_BORDER: u32 = 0x00800000;
const WS_VSCROLL: u32 = 0x00200000;
const WS_POPUP: u32 = 0x80000000;
const WS_CAPTION: u32 = 0x00c00000;
const WS_SYSMENU: u32 = 0x00080000;
const DS_MODALFRAME: u32 = 0x0080;
const DS_CENTER: u32 = 0x0800;
const DS_SETFONT: u32 = 0x0040;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const BS_OWNERDRAW: u32 = 0x000b;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const ATOM_BUTTON: u16 = 0x0080;
const ATOM_EDIT: u16 = 0x0081;
const ATOM_STATIC: u16 = 0x0082;
const ATOM_COMBOBOX: u16 = 0x0085;

fn build_template() -> Vec<u16> {
    let mut t = Tpl::new();
    t.dw(WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_CENTER | DS_SETFONT);
    t.dw(0);
    t.w(0);
    t.xy(0);
    t.xy(0);
    t.xy(348);
    t.xy(336);
    t.w(0);
    t.w(0);
    t.str("CloudreveLite 设置");
    t.w(9);
    t.str("Segoe UI Variable Text");

    let label = WS_CHILD | WS_VISIBLE;
    let edit = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL;
    let button = WS_CHILD | WS_VISIBLE | WS_TABSTOP;

    t.item(
        label,
        16,
        12,
        316,
        18,
        IDC_TITLE as u16,
        ATOM_STATIC,
        "连接与分享",
    );
    t.item(
        label,
        16,
        31,
        316,
        11,
        IDC_SUBTITLE as u16,
        ATOM_STATIC,
        "连接你的 CloudreveLite，并设置资源管理器右键分享的默认规则。",
    );
    t.item(label | SS_ETCHEDHORZ, 16, 49, 316, 1, 0, ATOM_STATIC, "");

    t.item(label, 16, 61, 316, 9, 0, ATOM_STATIC, "服务器地址");
    t.item(edit, 16, 73, 316, 17, IDC_SERVER as u16, ATOM_EDIT, "");
    t.item(label, 16, 98, 316, 9, 0, ATOM_STATIC, "配对码或 API Key");
    t.item(
        edit | ES_PASSWORD,
        16,
        110,
        242,
        17,
        IDC_KEY as u16,
        ATOM_EDIT,
        "",
    );
    t.item(
        button | BS_AUTOCHECKBOX,
        269,
        112,
        63,
        12,
        IDC_SHOW_KEY as u16,
        ATOM_BUTTON,
        "显示内容",
    );
    t.item(
        label,
        16,
        132,
        316,
        18,
        IDC_HINT as u16,
        ATOM_STATIC,
        "配对码来自网页端“设置 → 密码与安全 → 连接 CLI”，10 分钟内有效。",
    );
    t.item(label | SS_ETCHEDHORZ, 16, 155, 316, 1, 0, ATOM_STATIC, "");
    t.item(
        label,
        16,
        166,
        150,
        10,
        IDC_SECTION_SHARE as u16,
        ATOM_STATIC,
        "默认分享规则",
    );
    t.item(label, 16, 183, 84, 9, 0, ATOM_STATIC, "有效期");
    t.item(
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST | WS_VSCROLL,
        16,
        195,
        88,
        90,
        IDC_EXPIRE as u16,
        ATOM_COMBOBOX,
        "",
    );
    t.item(label, 118, 183, 64, 9, 0, ATOM_STATIC, "下载次数");
    t.item(
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_NUMBER,
        118,
        195,
        58,
        17,
        IDC_DOWNLOADS as u16,
        ATOM_EDIT,
        "",
    );
    t.item(
        button | BS_AUTOCHECKBOX,
        192,
        196,
        140,
        13,
        IDC_PRIVATE as u16,
        ATOM_BUTTON,
        "自动生成访问密码",
    );

    t.item(label | SS_ETCHEDHORZ, 16, 223, 316, 1, 0, ATOM_STATIC, "");
    t.item(
        label,
        16,
        233,
        64,
        11,
        IDC_SECTION_UPDATE as u16,
        ATOM_STATIC,
        "软件更新",
    );
    t.item(label, 86, 233, 246, 11, IDC_VERSION as u16, ATOM_STATIC, "");
    t.item(
        button | BS_AUTOCHECKBOX,
        16,
        251,
        222,
        13,
        IDC_AUTO_UPDATE as u16,
        ATOM_BUTTON,
        "自动检查、下载并在空闲时安装",
    );
    t.item(
        button | BS_OWNERDRAW,
        248,
        248,
        84,
        18,
        IDC_CHECK_UPDATE as u16,
        ATOM_BUTTON,
        "检查更新",
    );
    t.item(
        label,
        16,
        271,
        316,
        18,
        IDC_UPDATE_STATUS as u16,
        ATOM_STATIC,
        "",
    );
    t.item(label, 16, 294, 316, 14, IDC_STATUS as u16, ATOM_STATIC, "");
    t.item(
        button | BS_OWNERDRAW,
        16,
        313,
        74,
        18,
        IDC_TEST as u16,
        ATOM_BUTTON,
        "测试连接",
    );
    t.item(
        button | BS_DEFPUSHBUTTON | BS_OWNERDRAW,
        208,
        313,
        56,
        18,
        IDOK as u16,
        ATOM_BUTTON,
        "保存设置",
    );
    t.item(
        button | BS_OWNERDRAW,
        276,
        313,
        56,
        18,
        IDCANCEL as u16,
        ATOM_BUTTON,
        "取消",
    );
    t.buf[4] = t.items;
    t.buf
}

fn get_text(hwnd: HWND, id: i32) -> String {
    let mut buffer = [0u16; 512];
    let length = unsafe { GetDlgItemTextW(hwnd, id, &mut buffer) } as usize;
    String::from_utf16_lossy(&buffer[..length.min(buffer.len() - 1)])
}

fn set_text(hwnd: HWND, id: i32, value: &str) {
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id, windows::core::PCWSTR(util::wide(value).as_ptr()));
    }
}

fn set_status(hwnd: HWND, text: &str, kind: i32) {
    STATUS_KIND.store(kind, Ordering::Relaxed);
    set_text(hwnd, IDC_STATUS, text);
}

fn collect(hwnd: HWND) -> Result<(String, String, u32, u32, bool), String> {
    let server = get_text(hwnd, IDC_SERVER)
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !(server.starts_with("https://") || server.starts_with("http://")) || server.len() < 10 {
        return Err("服务器地址必须以 https:// 或 http:// 开头。".into());
    }
    let key = get_text(hwnd, IDC_KEY).trim().to_string();
    let selection =
        unsafe { SendDlgItemMessageW(hwnd, IDC_EXPIRE, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0
            as usize;
    let days = DAYS.get(selection).copied().unwrap_or(7);
    let downloads: u32 = get_text(hwnd, IDC_DOWNLOADS)
        .trim()
        .parse()
        .unwrap_or(10)
        .clamp(1, 100);
    let private =
        unsafe { SendDlgItemMessageW(hwnd, IDC_PRIVATE, BM_GETCHECK, WPARAM(0), LPARAM(0)) }.0
            == BST_CHECKED;
    Ok((server, key, days, downloads, private))
}

fn do_test(hwnd: HWND) {
    let (server, key, ..) = match collect(hwnd) {
        Ok(value) => value,
        Err(error) => return set_status(hwnd, &error, STATUS_ERROR),
    };
    set_status(hwnd, "正在测试连接…", STATUS_INFO);
    if let Err(error) = crate::api::api_call(&server, "", "GET", "/site/ping", None) {
        return set_status(
            hwnd,
            &format!("无法连接服务器：{}", error.msg),
            STATUS_ERROR,
        );
    }
    let saved = {
        let cfg = crate::CONFIG.read().unwrap();
        config::decrypt_key(&cfg.api_key_enc)
    };
    let effective = if key.starts_with("crl_") { key } else { saved };
    if effective.is_empty() {
        return set_status(hwnd, "服务器可达；保存配对码后即可上传。", STATUS_SUCCESS);
    }
    match crate::api::validate_key(&server, &effective) {
        Ok(_) => set_status(hwnd, "连接正常，API Key 有效。", STATUS_SUCCESS),
        Err(error) => set_status(hwnd, &format!("API Key 无效：{}", error.msg), STATUS_ERROR),
    }
}

fn do_save(hwnd: HWND) {
    let (server, key_field, days, downloads, private) = match collect(hwnd) {
        Ok(value) => value,
        Err(error) => return set_status(hwnd, &error, STATUS_ERROR),
    };
    let old_key = {
        let cfg = crate::CONFIG.read().unwrap();
        config::decrypt_key(&cfg.api_key_enc)
    };
    let key = if key_field.starts_with("crp_") {
        set_status(hwnd, "正在验证配对码…", STATUS_INFO);
        match crate::api::exchange_pairing(&server, &key_field) {
            Ok(key) => {
                util::log("paired new api key via pairing code");
                key
            }
            Err(error) => {
                return set_status(
                    hwnd,
                    &format!("配对码验证失败：{}", error.msg),
                    STATUS_ERROR,
                )
            }
        }
    } else if key_field.starts_with("crl_") {
        key_field
    } else if key_field.is_empty() {
        old_key
    } else {
        return set_status(
            hwnd,
            "请填写 crp_ 配对码、crl_ API Key，或留空保持当前连接。",
            STATUS_ERROR,
        );
    };

    let new_config = config::Config {
        server,
        api_key_enc: if key.is_empty() {
            String::new()
        } else {
            config::encrypt_key(&key)
        },
        expire_days: days,
        downloads,
        private,
        auto_update: unsafe {
            SendDlgItemMessageW(hwnd, IDC_AUTO_UPDATE, BM_GETCHECK, WPARAM(0), LPARAM(0)).0
                == BST_CHECKED
        },
    };
    if !config::save(&new_config) {
        return set_status(
            hwnd,
            "保存失败，请检查用户配置目录的写入权限。",
            STATUS_ERROR,
        );
    }
    *crate::CONFIG.write().unwrap() = new_config;
    util::log("settings saved");
    unsafe {
        let _ = EndDialog(hwnd, 1);
    }
}

unsafe fn ensure_fonts() -> (
    windows::Win32::Graphics::Gdi::HFONT,
    windows::Win32::Graphics::Gdi::HFONT,
    windows::Win32::Graphics::Gdi::HFONT,
) {
    if FONT_TITLE.load(Ordering::Relaxed) == 0 {
        FONT_TITLE.store(ui::font(-23, 600).0 as isize, Ordering::Relaxed);
        FONT_BODY.store(ui::font(-15, 400).0 as isize, Ordering::Relaxed);
        FONT_STRONG.store(ui::font(-15, 600).0 as isize, Ordering::Relaxed);
    }
    (
        windows::Win32::Graphics::Gdi::HFONT(FONT_TITLE.load(Ordering::Relaxed) as *mut _),
        windows::Win32::Graphics::Gdi::HFONT(FONT_BODY.load(Ordering::Relaxed) as *mut _),
        windows::Win32::Graphics::Gdi::HFONT(FONT_STRONG.load(Ordering::Relaxed) as *mut _),
    )
}

unsafe fn initialize(hwnd: HWND) {
    WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
    ui::set_dpi(GetDpiForWindow(hwnd));
    ui::apply_light_frame(hwnd);
    let (title_font, body_font, strong_font) = ensure_fonts();
    for id in [
        IDC_SERVER,
        IDC_KEY,
        IDC_EXPIRE,
        IDC_DOWNLOADS,
        IDC_PRIVATE,
        IDC_TEST,
        IDC_HINT,
        IDC_SUBTITLE,
        IDC_SHOW_KEY,
        IDC_STATUS,
        IDC_VERSION,
        IDC_AUTO_UPDATE,
        IDC_CHECK_UPDATE,
        IDC_UPDATE_STATUS,
        IDOK,
        IDCANCEL,
    ] {
        let control = GetDlgItem(Some(hwnd), id).unwrap_or_default();
        ui::apply_font(control, body_font);
        ui::theme(control);
    }
    ui::apply_font(
        GetDlgItem(Some(hwnd), IDC_TITLE).unwrap_or_default(),
        title_font,
    );
    ui::apply_font(
        GetDlgItem(Some(hwnd), IDC_SECTION_SHARE).unwrap_or_default(),
        strong_font,
    );
    ui::apply_font(
        GetDlgItem(Some(hwnd), IDC_SECTION_UPDATE).unwrap_or_default(),
        strong_font,
    );
    set_text(
        hwnd,
        IDC_VERSION,
        &format!("当前版本 v{}", crate::update::VERSION),
    );

    let cfg = crate::CONFIG.read().unwrap();
    set_text(hwnd, IDC_SERVER, &cfg.server);
    set_text(hwnd, IDC_DOWNLOADS, &cfg.downloads.to_string());
    let has_key = !cfg.api_key_enc.is_empty();
    let selection = DAYS
        .iter()
        .position(|day| *day == cfg.expire_days)
        .unwrap_or(2);
    let private = cfg.private;
    let auto_update = cfg.auto_update;
    drop(cfg);

    for day in DAYS {
        let text = format!("{day} 天");
        SendDlgItemMessageW(
            hwnd,
            IDC_EXPIRE,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(util::wide(&text).as_ptr() as isize),
        );
    }
    SendDlgItemMessageW(hwnd, IDC_EXPIRE, CB_SETCURSEL, WPARAM(selection), LPARAM(0));
    if private {
        SendDlgItemMessageW(hwnd, IDC_PRIVATE, BM_SETCHECK, WPARAM(1), LPARAM(0));
    }
    SendDlgItemMessageW(
        hwnd,
        IDC_AUTO_UPDATE,
        BM_SETCHECK,
        WPARAM(usize::from(auto_update)),
        LPARAM(0),
    );
    refresh_update(hwnd);
    windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), TIMER_UPDATE, 500, None);
    SendDlgItemMessageW(hwnd, IDC_KEY, EM_SETPASSWORDCHAR, WPARAM(0x25cf), LPARAM(0));
    let cue = if has_key {
        "已连接；留空会保留当前 API Key"
    } else {
        "粘贴网页端生成的 crp_ 配对码"
    };
    SendDlgItemMessageW(
        hwnd,
        IDC_KEY,
        EM_SETCUEBANNER,
        WPARAM(1),
        LPARAM(util::wide(cue).as_ptr() as isize),
    );
    if has_key {
        set_status(hwnd, "已连接；修改后保存即可生效。", STATUS_SUCCESS);
    } else {
        set_status(hwnd, "尚未连接，请先粘贴配对码。", STATUS_INFO);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match message {
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER if wparam.0 == TIMER_UPDATE => {
            refresh_update(hwnd);
            1
        }
        WM_INITDIALOG => {
            initialize(hwnd);
            1
        }
        WM_CTLCOLORDLG => ui::background_brush().0 as isize,
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut _);
            let control = HWND(lparam.0 as *mut _);
            let id = GetDlgCtrlID(control);
            let color = match id {
                IDC_TITLE | IDC_SECTION_SHARE | IDC_SECTION_UPDATE => ui::TEXT,
                IDC_SUBTITLE | IDC_HINT | IDC_VERSION | IDC_UPDATE_STATUS => ui::MUTED,
                IDC_STATUS => match STATUS_KIND.load(Ordering::Relaxed) {
                    STATUS_INFO => ui::ACCENT,
                    STATUS_SUCCESS => ui::SUCCESS,
                    STATUS_ERROR => ui::ERROR,
                    _ => ui::MUTED,
                },
                _ => ui::TEXT,
            };
            let _ = SetTextColor(hdc, color);
            let _ = SetBkMode(hdc, TRANSPARENT);
            ui::background_brush().0 as isize
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            if code == 0 {
                match id {
                    IDC_SHOW_KEY => {
                        let checked = SendDlgItemMessageW(
                            hwnd,
                            IDC_SHOW_KEY,
                            BM_GETCHECK,
                            WPARAM(0),
                            LPARAM(0),
                        )
                        .0 == BST_CHECKED;
                        SendDlgItemMessageW(
                            hwnd,
                            IDC_KEY,
                            EM_SETPASSWORDCHAR,
                            WPARAM(if checked { 0 } else { 0x25cf }),
                            LPARAM(0),
                        );
                    }
                    IDC_TEST => do_test(hwnd),
                    IDC_CHECK_UPDATE => {
                        crate::update::check(true);
                        refresh_update(hwnd);
                    }
                    IDOK => do_save(hwnd),
                    IDCANCEL => {
                        let _ = EndDialog(hwnd, 0);
                    }
                    _ => {}
                }
            }
            1
        }
        WM_DRAWITEM => {
            if lparam.0 != 0 {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if matches!(
                    item.CtlID as i32,
                    IDC_TEST | IDC_CHECK_UPDATE | IDOK | IDCANCEL
                ) {
                    ui::draw_button(item, item.CtlID as i32 == IDOK);
                    return 1;
                }
            }
            0
        }
        WM_CLOSE => {
            let _ = EndDialog(hwnd, 0);
            1
        }
        _ => 0,
    }
}

pub fn open(owner: Option<HWND>) -> bool {
    let template = build_template();
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
        .unwrap_or_default();
    let result = unsafe {
        DialogBoxIndirectParamW(
            Some(windows::Win32::Foundation::HINSTANCE(module.0)),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc),
            LPARAM(0),
        )
    };
    WINDOW.store(0, Ordering::Relaxed);
    result != 0 && result != -1
}

fn refresh_update(hwnd: HWND) {
    let (text, busy) = crate::update::status();
    set_text(
        hwnd,
        IDC_UPDATE_STATUS,
        if text.is_empty() {
            "手动检查可下载并安装新版本；正在上传时会延后重启。"
        } else {
            &text
        },
    );
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(
            GetDlgItem(Some(hwnd), IDC_CHECK_UPDATE).unwrap_or_default(),
            !busy,
        );
    }
}

pub fn close_for_update() {
    let hwnd = WINDOW.load(Ordering::Relaxed);
    if hwnd != 0 {
        unsafe {
            let _ = EndDialog(HWND(hwnd as *mut _), 0);
        }
    }
}
