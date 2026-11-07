//! Icons come from the current executable, never an external file or DLL.
use std::{ptr::null, sync::OnceLock};
use windows_sys::Win32::{System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};
pub(crate) fn application_icon(small: bool) -> HICON {
    static SMALL: OnceLock<usize> = OnceLock::new();
    static LARGE: OnceLock<usize> = OnceLock::new();
    // Unshared, size-specific handles remain owned for the process lifetime.
    *(if small { &SMALL } else { &LARGE }).get_or_init(|| unsafe {
        let (x, y) = if small {
            (SM_CXSMICON, SM_CYSMICON)
        } else {
            (SM_CXICON, SM_CYICON)
        };
        let icon = LoadImageW(
            GetModuleHandleW(null()),
            1usize as *const u16,
            IMAGE_ICON,
            GetSystemMetrics(x),
            GetSystemMetrics(y),
            0,
        );
        if icon.is_null() {
            LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) as usize
        } else {
            icon as usize
        }
    }) as HICON
}
