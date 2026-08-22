//! IExplorerCommand shell extension for the CloudreveLite tray app.
//! Registered per-user (HKCU) for the classic context menu and via a sparse
//! MSIX package for the Windows 11 modern menu. Invoking the command sends
//! the selected file paths over the tray app's named pipe.

mod command;
mod pipe_client;

use command::{CloudreveCommand, CLSID_CLOUDREVE_CMD};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::IExplorerCommand;

#[implement(IClassFactory)]
struct CloudreveClassFactory;

impl IClassFactory_Impl for CloudreveClassFactory_Impl {
    fn CreateInstance(
        &self,
        _punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut std::ffi::c_void,
    ) -> Result<()> {
        let cmd: IExplorerCommand = CloudreveCommand.into();
        unsafe {
            let hr = cmd.query(&*riid, ppvobject);
            hr.ok()
        }
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
extern "system" fn DllMain(_hinstance: HINSTANCE, reason: u32, _reserved: *mut std::ffi::c_void) -> BOOL {
    let _ = reason;
    TRUE
}

#[no_mangle]
extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> HRESULT {
    unsafe {
        if *rclsid == CLSID_CLOUDREVE_CMD {
            let factory: IClassFactory = CloudreveClassFactory.into();
            factory.query(&*riid, ppv)
        } else {
            CLASS_E_CLASSNOTAVAILABLE
        }
    }
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
