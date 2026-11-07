//! Native tray and process lifetime. Never hides a window without a reachable icon.
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::*,
    System::Threading::CreateMutexW,
    UI::{Shell::*, WindowsAndMessaging::*},
};
pub const TRAY_MESSAGE: u32 = WM_APP + 41;
pub const REOPEN_MESSAGE: u32 = WM_APP + 42;
pub const REOPEN: usize = 150;
pub const EXIT: usize = 151;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Instance {
    /// Claim before data initialization. Existing instances are asked to reopen only.
    pub fn claim(name: &str) -> Result<Option<Self>, String> {
        let handle = unsafe { CreateMutexW(null(), 0, wide(name).as_ptr()) };
        if handle.is_null() {
            return Err("Cannot establish single application instance".into());
        }
        let exists = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
        if exists {
            unsafe { CloseHandle(handle) };
            Ok(None)
        } else {
            Ok(Some(Self(handle)))
        }
    }
    pub fn product() -> Result<Option<Self>, String> {
        let instance = Self::claim("Local\\EnvironmentManager.Application")?;
        if instance.is_none() {
            for _ in 0..40 {
                let hwnd = unsafe { FindWindowW(wide("EnvironmentManager.Main").as_ptr(), null()) };
                if !hwnd.is_null() {
                    unsafe {
                        PostMessageW(hwnd, REOPEN_MESSAGE, 0, 0);
                        SetForegroundWindow(hwnd);
                    };
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Ok(instance)
    }
}
pub struct Tray {
    data: NOTIFYICONDATAW,
    installed: bool,
    interrupted: Option<bool>,
    taskbar_created: u32,
}
impl Tray {
    pub unsafe fn new(hwnd: HWND) -> Self {
        let mut data = NOTIFYICONDATAW::default();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        Self {
            data,
            installed: false,
            interrupted: None,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        }
    }
    pub fn taskbar_message(&self) -> u32 {
        self.taskbar_created
    }
    pub unsafe fn update(&mut self, interrupted: bool) -> bool {
        if self.installed && self.interrupted == Some(interrupted) {
            return true;
        }
        self.data.hIcon = if interrupted {
            LoadIconW(null_mut(), IDI_ERROR)
        } else {
            crate::icons::application_icon(true)
        };
        self.data.szTip.fill(0);
        let tip = wide(if interrupted {
            "Environment Manager - recording interrupted"
        } else {
            "Environment Manager - monitoring"
        });
        self.data.szTip[..tip.len()].copy_from_slice(&tip);
        let ok = Shell_NotifyIconW(
            if self.installed { NIM_MODIFY } else { NIM_ADD },
            &self.data,
        ) != 0;
        if ok && !self.installed {
            self.data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &self.data);
        }
        self.installed = ok;
        self.interrupted = Some(interrupted);
        ok
    }
    pub unsafe fn recover(&mut self, interrupted: bool) -> bool {
        self.installed = false;
        self.update(interrupted)
    }
    pub unsafe fn menu(&self) -> usize {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return REOPEN;
        }
        AppendMenuW(
            menu,
            MF_STRING,
            REOPEN,
            wide("&Open Environment Manager").as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, EXIT, wide("E&xit").as_ptr());
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        SetForegroundWindow(self.data.hWnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            self.data.hWnd,
            null(),
        );
        DestroyMenu(menu);
        PostMessageW(self.data.hWnd, WM_NULL, 0, 0);
        command as usize
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        if self.installed {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.data);
            }
        }
    }
}
