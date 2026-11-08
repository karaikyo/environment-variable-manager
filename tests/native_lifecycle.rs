use environment_variable_manager::{
    desktop,
    startup::{RegistrationState, StartupRegistration},
    RegistryReader, RegistryWriter, Scope, Variable,
};
use std::{
    ptr::null,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::*, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
};
#[path = "support/entry_refresh.rs"]
mod entry_refresh;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe extern "system" fn reject_window(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        0
    } else {
        DefWindowProcW(hwnd, message, w, l)
    }
}
struct Reader;
impl RegistryReader for Reader {
    fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
        Ok(vec![])
    }
}
impl RegistryWriter for Reader {
    fn write_user(&self, _: &Variable) -> Result<(), String> {
        panic!("No environment write is permitted")
    }
}
struct Registration(Arc<AtomicBool>);
impl StartupRegistration for Registration {
    fn state(&self) -> Result<RegistrationState, String> {
        Ok(RegistrationState::default())
    }
    fn replace(&self, _: Option<&str>, _: Option<&str>) -> Result<(), String> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}
#[test]
fn failed_native_initialization_exits_without_registering_startup_or_leaving_worker_alive() {
    let directory = std::env::temp_dir().join(format!(
        "evm-window-failure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).expect("Exclusively reserve owned disposable test data");
    let registered = Arc::new(AtomicBool::new(false));
    let class = wide("EnvironmentManager.Main");
    let instance = unsafe { GetModuleHandleW(null()) };
    let atom = unsafe {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(reject_window),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        })
    };
    assert_ne!(atom, 0, "Never replace an existing window class");
    let result = desktop::run_with_startup(
        Reader,
        directory.clone(),
        Registration(registered.clone()),
        std::path::PathBuf::from("C:/Owned/app.exe"),
        false,
    );
    unsafe {
        assert_ne!(UnregisterClassW(class.as_ptr(), instance), 0);
    }
    assert!(result.is_err());
    let startup_was_registered = registered.load(Ordering::SeqCst);
    let original = directory.canonicalize().unwrap();
    assert!(original.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    std::fs::remove_dir_all(original).unwrap();
    assert!(!startup_was_registered);
}
