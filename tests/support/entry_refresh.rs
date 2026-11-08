//! Exercise observation while editing through the real window, in an owned child.
use environment_variable_manager::{
    desktop,
    startup::{RegistrationState, StartupRegistration},
    RegistryReader, RegistryWriter, Scope, Variable,
};
use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    UI::{Controls::*, Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
};
#[path = "../../examples/support/native_status.rs"]
mod native_status;

struct Fixture(Arc<Mutex<String>>);
impl RegistryReader for Fixture {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(if scope == Scope::User {
            vec![Variable::text(
                scope,
                "OwnedFixture",
                &self.0.lock().unwrap(),
                false,
            )]
        } else {
            vec![]
        })
    }
}
impl RegistryWriter for Fixture {
    fn write_user(&self, _: &Variable) -> Result<(), String> {
        panic!("This fixture must never write environment variables")
    }
}
#[derive(Default)]
struct Registration(Mutex<RegistrationState>);
impl StartupRegistration for Registration {
    fn state(&self) -> Result<RegistrationState, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        assert_eq!(state.command.as_deref(), expected);
        state.command = desired.map(str::to_owned);
        Ok(())
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "Owned window did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
}
unsafe fn text(window: HWND) -> String {
    let mut buffer = vec![0; GetWindowTextLengthW(window) as usize + 1];
    let length = GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..length as usize])
}
unsafe fn select(window: HWND, index: usize) {
    let item = LVITEMW {
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        state: LVIS_SELECTED | LVIS_FOCUSED,
        ..Default::default()
    };
    SendMessageW(window, LVM_SETITEMSTATE, index, &item as *const _ as isize);
}

#[test]
fn automatic_observation_preserves_active_entry_and_editor_selection() {
    let directory = std::env::temp_dir().join(format!(
        "evm-entry-refresh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).expect("Exclusively reserve owned fixture");
    let log = std::fs::File::create(directory.join("child.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "entry_refresh::observation_child",
            "--ignored",
            "--nocapture",
        ])
        .env("EVM_ENTRY_REFRESH_FIXTURE", &directory)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Owned fixture timed out; evidence: {}", directory.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "Native entry regression: {}\n{}",
        directory.display(),
        std::fs::read_to_string(directory.join("child.log")).unwrap()
    );
    let owned = directory.canonicalize().unwrap();
    assert!(owned.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    std::fs::remove_dir_all(owned).unwrap();
}

#[test]
#[ignore = "executed only by the owned native child harness"]
fn observation_child() {
    let directory = PathBuf::from(std::env::var_os("EVM_ENTRY_REFRESH_FIXTURE").unwrap());
    let value = Arc::new(Mutex::new("first;second;third".to_owned()));
    let external = value.clone();
    let controller = std::thread::spawn(move || unsafe {
        let mut window = std::ptr::null_mut();
        wait(|| {
            window =
                native_status::find_owned_window("EnvironmentManager.Main", std::process::id());
            !window.is_null() && native_status::healthy(window)
        });
        let result = std::panic::catch_unwind(|| {
            let entries = GetDlgItem(window, 104);
            let editor = GetDlgItem(window, 105);
            select(entries, 1);
            // A real entry command moves the row and focuses its editor on the UI thread.
            SendMessageW(window, WM_COMMAND, 109, 0);
            assert_eq!(text(editor), "second");
            let draft: Vec<u16> = "second-unsaved".encode_utf16().chain(Some(0)).collect();
            SendMessageW(editor, EM_SETSEL, 0, -1);
            SendMessageW(editor, EM_REPLACESEL, 1, draft.as_ptr() as isize);
            SendMessageW(editor, EM_SETSEL, 2, 5);
            assert_ne!(IsWindowEnabled(GetDlgItem(window, 110)), 0);
            // The status is now the draft message; only a completed response restores healthy status.
            assert!(!native_status::healthy(window));
            SendMessageW(window, WM_SETTINGCHANGE, 0, 0);
            wait(|| native_status::healthy(window));
            assert_eq!(
                SendMessageW(entries, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize),
                2
            );
            assert_eq!(text(editor), "second-unsaved");
            let (mut start, mut end) = (0u32, 0u32);
            SendMessageW(
                editor,
                EM_GETSEL,
                &mut start as *mut _ as usize,
                &mut end as *mut _ as isize,
            );
            assert_eq!((start, end), (2, 5));
            if GetForegroundWindow() == window {
                let mut info = GUITHREADINFO {
                    cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                assert_ne!(
                    GetGUIThreadInfo(
                        GetWindowThreadProcessId(window, std::ptr::null_mut()),
                        &mut info
                    ),
                    0
                );
                assert_eq!(info.hwndFocus, editor);
            }
            select(entries, 0);
            assert_eq!(text(editor), "first");
            select(entries, 2);
            assert_eq!(text(editor), "second-unsaved");
            SendMessageW(window, WM_COMMAND, 111, 0);
            wait(|| {
                IsWindowEnabled(editor) != 0
                    && SendMessageW(entries, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize)
                        == 0
            });
            assert_eq!(text(editor), "first");
            select(entries, 2);
            assert_eq!(text(editor), "third");
            assert_eq!(
                GetWindowLongPtrW(editor, GWL_STYLE) & ES_READONLY as isize,
                0
            );
            *external.lock().unwrap() = "first".into();
            SendMessageW(window, WM_SETTINGCHANGE, 0, 0);
            wait(|| {
                native_status::healthy(window) && SendMessageW(entries, LVM_GETITEMCOUNT, 0, 0) == 1
            });
            assert_eq!(
                SendMessageW(entries, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize),
                -1
            );
            assert_eq!(text(editor), "");
            assert_ne!(
                GetWindowLongPtrW(editor, GWL_STYLE) & ES_READONLY as isize,
                0
            );
            select(entries, 0);
            assert_eq!(text(editor), "first");
            assert_eq!(
                GetWindowLongPtrW(editor, GWL_STYLE) & ES_READONLY as isize,
                0
            );
        });
        SendMessageW(window, WM_COMMAND, 151, 0);
        wait(|| {
            IsWindow(window) == 0
                || (IsWindowVisible(GetDlgItem(window, 153)) != 0
                    && IsWindowEnabled(GetDlgItem(window, 153)) != 0)
        });
        if IsWindow(window) != 0 {
            SendMessageW(window, WM_COMMAND, 153, 0);
        }
        result
    });
    desktop::run_with_startup(
        Fixture(value),
        directory.join("data"),
        Registration::default(),
        PathBuf::from("C:/Owned/app.exe"),
        false,
    )
    .unwrap();
    if let Err(error) = controller.join().unwrap() {
        std::panic::resume_unwind(error);
    }
}
