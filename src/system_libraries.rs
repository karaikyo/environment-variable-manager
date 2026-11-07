//! Optional non-KnownDLL dependencies are loaded only from System32, after main.
use std::ptr::null_mut;
use windows_sys::Win32::{Foundation::HMODULE, System::LibraryLoader::*, UI::Controls::*};
struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::FreeLibrary(self.0);
        }
    }
}
fn load_system(name: &str) -> Result<Library, String> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let module = unsafe { LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if module.is_null() {
        return Err("Cannot load required Windows system library".into());
    }
    Ok(Library(module))
}
pub(crate) fn random_bytes(bytes: &mut [u8]) -> Result<(), String> {
    let library = load_system("bcrypt.dll")?;
    let function = unsafe { GetProcAddress(library.0, c"BCryptGenRandom".as_ptr().cast()) }
        .ok_or("Windows randomness unavailable")?;
    let function: unsafe extern "system" fn(*mut core::ffi::c_void, *mut u8, u32, u32) -> i32 =
        unsafe { std::mem::transmute(function) };
    if unsafe {
        function(
            null_mut(),
            bytes.as_mut_ptr(),
            bytes
                .len()
                .try_into()
                .map_err(|_| "Oversized random buffer")?,
            2,
        )
    } < 0
    {
        return Err("Windows randomness failed".into());
    }
    Ok(())
}
pub(crate) fn init_controls() -> Result<(), String> {
    static CONTROLS: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    CONTROLS
        .get_or_init(|| {
            let library = load_system("comctl32.dll")?;
            let function =
                unsafe { GetProcAddress(library.0, c"InitCommonControlsEx".as_ptr().cast()) }
                    .ok_or("Windows common controls unavailable")?;
            let function: unsafe extern "system" fn(*const INITCOMMONCONTROLSEX) -> i32 =
                unsafe { std::mem::transmute(function) };
            let controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                // Button/edit/static/combobox classes are intrinsic to User32.
                // Register only the common-controls classes this app uses.
                dwICC: ICC_LISTVIEW_CLASSES,
            };
            if unsafe { function(&controls) } == 0 {
                return Err("Cannot initialize Windows common controls".into());
            }
            let module = library.0 as usize;
            // Window class procedures remain in this module for the process lifetime.
            std::mem::forget(library);
            Ok(module)
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
