//! Owned native update controls; routine tests never contact GitHub or launch a browser.
use environment_variable_manager::{
    desktop,
    startup::{RegistrationState, StartupRegistration},
    RegistryReader, RegistryWriter, Scope, Variable,
};
use std::{
    path::PathBuf,
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    UI::{Controls::*, Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
};
#[path = "../../examples/support/native_status.rs"]
mod native_status;
struct Registry;
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(if scope == Scope::User {
            vec![Variable::text(
                scope,
                "OwnedFixture",
                "first;second;third",
                false,
            )]
        } else {
            vec![]
        })
    }
}
impl RegistryWriter for Registry {
    fn write_user(&self, _: &Variable) -> Result<(), String> {
        panic!("This fixture never writes environment variables")
    }
}
#[derive(Default)]
struct Registration(Mutex<RegistrationState>);
impl StartupRegistration for Registration {
    fn state(&self) -> Result<RegistrationState, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        assert_eq!(s.command.as_deref(), expected);
        s.command = desired.map(str::to_owned);
        Ok(())
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "Owned update window timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
unsafe fn text(window: HWND) -> String {
    let mut b = vec![0u16; GetWindowTextLengthW(window) as usize + 1];
    let n = GetWindowTextW(window, b.as_mut_ptr(), b.len() as i32);
    String::from_utf16_lossy(&b[..n as usize])
}
fn run_child(test: &str) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "evm-update-native-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).expect("Exclusively reserve update fixture");
    let log = std::fs::File::create(dir.join("child.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture"])
        .env("EVM_UPDATE_NATIVE_DATA", &dir)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Owned child timed out: {}", dir.display())
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "Update fixture failed: {}\n{}",
        dir.display(),
        std::fs::read_to_string(dir.join("child.log")).unwrap()
    );
    let owned = dir.canonicalize().unwrap();
    assert!(owned.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    std::fs::remove_dir_all(owned).unwrap();
}
#[test]
fn manual_update_controls_start_idle() {
    run_child("update_check_native::native_controls_child")
}
#[test]
#[ignore = "owned child invoked by parent"]
fn native_controls_child() {
    let dir = PathBuf::from(std::env::var_os("EVM_UPDATE_NATIVE_DATA").unwrap());
    let driver = std::thread::spawn(|| unsafe {
        let mut window = std::ptr::null_mut();
        wait(|| {
            window =
                native_status::find_owned_window("EnvironmentManager.Main", std::process::id());
            !window.is_null() && native_status::healthy(window)
        });
        let result = std::panic::catch_unwind(|| {
            assert!(
                !GetDlgItem(window, 160).is_null(),
                "Check for updates control missing"
            );
            assert_eq!(
                text(GetDlgItem(window, 163)),
                format!("Version {}", env!("CARGO_PKG_VERSION"))
            );
            assert_ne!(IsWindowEnabled(GetDlgItem(window, 160)), 0);
            assert_eq!(IsWindowVisible(GetDlgItem(window, 161)), 0);
            assert_eq!(
                text(GetDlgItem(window, 162)),
                "Updates have not been checked."
            );
        });
        SendMessageW(window, WM_COMMAND, 151, 0);
        result
    });
    desktop::run_with_startup(
        Registry,
        dir.join("data"),
        Registration::default(),
        PathBuf::from("C:/Owned/app.exe"),
        false,
    )
    .unwrap();
    if let Err(e) = driver.join().unwrap() {
        std::panic::resume_unwind(e)
    }
}
