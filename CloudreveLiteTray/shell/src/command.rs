use crate::pipe_client;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

// {AA8A460B-1449-4FA6-B339-B01B527F7FF5}
pub const CLSID_CLOUDREVE_CMD: GUID = GUID::from_u128(0xAA8A460B_1449_4FA6_B339_B01B527F7FF5);

#[implement(IExplorerCommand)]
pub struct CloudreveCommand;

impl IExplorerCommand_Impl for CloudreveCommand_Impl {
    fn GetTitle(&self, _psiitemarray: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        alloc_pwstr("上传到 CloudreveLite 并分享")
    }

    fn GetIcon(&self, _psiitemarray: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        match pipe_client::installed_app_icon() {
            Some(path) => alloc_pwstr(&format!("{path},0")),
            None => alloc_pwstr("%SystemRoot%\\System32\\shell32.dll,-16817"),
        }
    }

    fn GetToolTip(&self, _psiitemarray: Ref<'_, IShellItemArray>) -> Result<PWSTR> {
        alloc_pwstr("上传文件到 CloudreveLite 并复制分享链接")
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(CLSID_CLOUDREVE_CMD)
    }

    fn GetState(
        &self,
        _psiitemarray: Ref<'_, IShellItemArray>,
        _foktobeslow: BOOL,
    ) -> Result<u32> {
        // ECS_ENABLED = 0
        Ok(0)
    }

    fn GetFlags(&self) -> Result<u32> {
        // ECF_DEFAULT = 0
        Ok(0)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(
        &self,
        psiitemarray: Ref<'_, IShellItemArray>,
        _pbc: Ref<'_, IBindCtx>,
    ) -> Result<()> {
        let items = match psiitemarray.ok() {
            Ok(items) => items,
            Err(_) => return Ok(()),
        };

        let count = unsafe { items.GetCount()? };
        let mut files = Vec::new();

        for i in 0..count {
            let item = unsafe { items.GetItemAt(i)? };
            let path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
            let path_str = unsafe { path.to_string() }.map_err(|_| Error::from(E_FAIL))?;
            unsafe { CoTaskMemFree(Some(path.0 as *const _)) };
            files.push(path_str);
        }

        if !files.is_empty() {
            if pipe_client::send_upload_request(&files).is_err() {
                pipe_client::try_launch_main_process();
                let _ = pipe_client::send_upload_request(&files);
            }
        }

        Ok(())
    }
}

/// Allocate a wide string via CoTaskMemAlloc; the caller (Explorer) frees it.
fn alloc_pwstr(s: &str) -> Result<PWSTR> {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let byte_len = wide.len() * 2;
    let ptr = unsafe { CoTaskMemAlloc(byte_len) as *mut u16 };
    if ptr.is_null() {
        return Err(E_OUTOFMEMORY.into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
    }
    Ok(PWSTR(ptr))
}
