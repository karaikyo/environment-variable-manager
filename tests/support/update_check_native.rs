//! Owned native update controls; routine tests never contact GitHub or launch a browser.
use environment_variable_manager::{
    desktop,
    startup::{RegistrationState, StartupRegistration},
    RegistryReader, RegistryWriter, Scope, Variable,
};
use environment_variable_manager::{
    update_check::{
        CheckControl, HttpResponse, Release, ReleaseTransport, UpdateChecker, CURRENT_VERSION,
    },
    Application,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::{
    path::PathBuf,
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Replies {
    calls: AtomicUsize,
    mode: AtomicUsize,
    active: AtomicUsize,
}
impl ReleaseTransport for Replies {
    fn fetch(&self, _: &str, control: &CheckControl) -> Result<HttpResponse, String> {
        struct Active<'a>(&'a AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(&self.active);
        while self.mode.load(Ordering::SeqCst) == 0 {
            control.check()?;
            std::thread::sleep(Duration::from_millis(5));
        }
        control.check()?;
        if self.mode.load(Ordering::SeqCst) == 2 {
            return Err("Owned fixture connection failure".into());
        }
        let version = if self.mode.load(Ordering::SeqCst) == 3 {
            CURRENT_VERSION
        } else {
            "999.0.0"
        };
        let body = format!(r#"[{{"tag_name":"v{version}","draft":false,"prerelease":true,"published_at":"2026-01-01T00:00:00Z","assets":[{{"name":"environment-variable-manager-{version}-windows-x64.zip","state":"uploaded"}}]}}]"#).into_bytes();
        Ok(HttpResponse {
            status: 200,
            body,
            link: None,
        })
    }
}
#[derive(Default)]
struct Browser {
    opened: Mutex<Vec<String>>,
    fail: AtomicBool,
}
struct BrowserAdapter(Arc<Browser>);
impl desktop::BrowserOpener for BrowserAdapter {
    fn open(&self, release: &Release) -> Result<(), String> {
        self.0.opened.lock().unwrap().push(release.url().into());
        if self.0.fail.load(Ordering::SeqCst) {
            Err("Owned browser failure".into())
        } else {
            Ok(())
        }
    }
}
struct MutableRegistry(Arc<Mutex<Vec<Variable>>>);
impl RegistryReader for MutableRegistry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
impl RegistryWriter for MutableRegistry {
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        *self
            .0
            .lock()
            .unwrap()
            .iter_mut()
            .find(|v| v.name == value.name)
            .unwrap() = value.clone();
        Ok(())
    }
}
unsafe fn select(window: HWND, index: usize) {
    let item = LVITEMW {
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        state: LVIS_SELECTED | LVIS_FOCUSED,
        ..Default::default()
    };
    SendMessageW(window, LVM_SETITEMSTATE, index, &item as *const _ as isize);
}
unsafe fn exit(window: HWND) {
    SendMessageW(window, WM_COMMAND, 151, 0);
    wait(|| {
        IsWindow(window) == 0
            || (IsWindowVisible(GetDlgItem(window, 153)) != 0
                && IsWindowEnabled(GetDlgItem(window, 153)) != 0)
    });
    if IsWindow(window) != 0 {
        SendMessageW(window, WM_COMMAND, 153, 0);
    }
}

#[test]
fn manual_checks_preserve_editing_observation_and_tray_state() {
    run_child("update_check_native::native_check_interaction_child");
}
#[test]
#[ignore = "owned child invoked by parent"]
fn native_check_interaction_child() {
    let directory = PathBuf::from(std::env::var_os("EVM_UPDATE_NATIVE_DATA").unwrap()).join("data");
    let values = Arc::new(Mutex::new(vec![
        Variable::text(Scope::User, "OwnedFixture", "first;second;third", false),
        Variable::text(Scope::User, "ZExternal", "before", false),
    ]));
    let query = Application::open(&MutableRegistry(values.clone()), &directory);
    let replies = Arc::new(Replies::default());
    let responses = replies.clone();
    let browser = Arc::new(Browser::default());
    let opened = browser.clone();
    let registry = values.clone();
    let driver = std::thread::spawn(move || unsafe {
        let mut window = std::ptr::null_mut();
        wait(|| {
            window =
                native_status::find_owned_window("EnvironmentManager.Main", std::process::id());
            !window.is_null() && native_status::healthy(window)
        });
        let result = std::panic::catch_unwind(|| {
            assert_eq!(responses.calls.load(Ordering::SeqCst), 0);
            let entries = GetDlgItem(window, 104);
            let editor = GetDlgItem(window, 105);
            select(entries, 2);
            SendMessageW(window, WM_COMMAND, 160, 0);
            wait(|| responses.calls.load(Ordering::SeqCst) == 1);
            SendMessageW(window, WM_COMMAND, 160, 0);
            assert_eq!(responses.calls.load(Ordering::SeqCst), 1);
            assert_eq!(text(GetDlgItem(window, 162)), "Checking for updates...");
            assert_eq!(IsWindowEnabled(GetDlgItem(window, 160)), 0);
            assert_ne!(IsWindowEnabled(editor), 0);
            let draft: Vec<u16> = "third-draft".encode_utf16().chain(Some(0)).collect();
            SendMessageW(editor, EM_SETSEL, 0, -1);
            SendMessageW(editor, EM_REPLACESEL, 1, draft.as_ptr() as isize);
            SendMessageW(editor, EM_SETSEL, 2, 5);
            *registry
                .lock()
                .unwrap()
                .iter_mut()
                .find(|v| v.name == "ZExternal")
                .unwrap() = Variable::text(Scope::User, "ZExternal", "after", false);
            SendMessageW(window, WM_SETTINGCHANGE, 0, 0);
            wait(|| {
                query
                    .history(Some((Scope::User, "ZExternal")), 0, 10)
                    .unwrap()
                    .len()
                    == 1
                    && IsWindowEnabled(GetDlgItem(window, 113)) != 0
            });
            assert_eq!(
                responses.active.load(Ordering::SeqCst),
                1,
                "Observation must finish while HTTP is pending"
            );
            responses.mode.store(1, Ordering::SeqCst);
            wait(|| text(GetDlgItem(window, 162)).starts_with("New version available: 999.0.0"));
            assert_eq!(text(editor), "third-draft");
            assert_eq!(
                SendMessageW(entries, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize),
                2
            );
            let (mut start, mut end) = (0u32, 0u32);
            SendMessageW(
                editor,
                EM_GETSEL,
                &mut start as *mut _ as usize,
                &mut end as *mut _ as isize,
            );
            assert_eq!((start, end), (2, 5));
            assert_eq!(
                query.history(None, 0, 50).unwrap().len(),
                1,
                "Only the external edit is history"
            );
            assert!(opened.opened.lock().unwrap().is_empty());
            SendMessageW(window, WM_COMMAND, 161, 0);
            assert_eq!(*opened.opened.lock().unwrap(),vec!["https://github.com/karaikyo/environment-variable-manager/releases/tag/v999.0.0"]);
            opened.fail.store(true, Ordering::SeqCst);
            SendMessageW(window, WM_COMMAND, 161, 0);
            assert_eq!(
                text(GetDlgItem(window, 162)),
                "Cannot open the release page; try again."
            );
            responses.mode.store(0, Ordering::SeqCst);
            SendMessageW(window, WM_COMMAND, 160, 0);
            wait(|| responses.calls.load(Ordering::SeqCst) == 2);
            assert_eq!(
                IsWindowVisible(GetDlgItem(window, 161)),
                0,
                "Retry hides stale release action"
            );
            SendMessageW(window, WM_CLOSE, 0, 0);
            assert_eq!(IsWindowVisible(window), 0);
            responses.mode.store(2, Ordering::SeqCst);
            wait(|| text(GetDlgItem(window, 162)).contains("Owned fixture connection failure"));
            assert_eq!(
                IsWindowVisible(window),
                0,
                "Check completion must not reopen tray window"
            );
            SendMessageW(
                window,
                environment_variable_manager::native_lifecycle::REOPEN_MESSAGE,
                0,
                0,
            );
            wait(|| IsWindowEnabled(GetDlgItem(window, 113)) != 0);
            assert_eq!(responses.calls.load(Ordering::SeqCst), 2);
            assert!(text(GetDlgItem(window, 162)).starts_with("Check failed:"));
            assert_eq!(text(editor), "third-draft");
            responses.mode.store(3, Ordering::SeqCst);
            SendMessageW(window, WM_COMMAND, 160, 0);
            wait(|| text(GetDlgItem(window, 162)) == "Up to date.");
            assert_eq!(IsWindowVisible(GetDlgItem(window, 161)), 0);
            assert_eq!(query.history(None, 0, 50).unwrap().len(), 1);
        });
        exit(window);
        result
    });
    desktop::run_with_services(
        MutableRegistry(values),
        directory,
        Registration::default(),
        PathBuf::from("C:/Owned/app.exe"),
        false,
        UpdateChecker::new(replies),
        BrowserAdapter(browser),
    )
    .unwrap();
    if let Err(e) = driver.join().unwrap() {
        std::panic::resume_unwind(e)
    }
}

#[test]
fn checks_are_manual_and_exit_cancels_after_draft_decision() {
    run_child("update_check_native::native_check_lifecycle_child");
}
#[test]
#[ignore = "owned child invoked by parent"]
fn native_check_lifecycle_child() {
    let directory = PathBuf::from(std::env::var_os("EVM_UPDATE_NATIVE_DATA").unwrap()).join("data");
    let replies = Arc::new(Replies::default());
    let responses = replies.clone();
    let browser = Arc::new(Browser::default());
    let database = directory.join("environment.sqlite");
    let driver = std::thread::spawn(move || unsafe {
        let mut window = std::ptr::null_mut();
        wait(|| {
            window =
                native_status::find_owned_window("EnvironmentManager.Main", std::process::id());
            !window.is_null() && native_status::healthy(window)
        });
        let result = std::panic::catch_unwind(|| {
            assert_eq!(IsWindowVisible(window), 0, "Fixture starts in tray");
            assert_eq!(responses.calls.load(Ordering::SeqCst), 0);
            SendMessageW(
                window,
                environment_variable_manager::native_lifecycle::REOPEN_MESSAGE,
                0,
                0,
            );
            wait(|| IsWindowVisible(window) != 0 && IsWindowEnabled(GetDlgItem(window, 113)) != 0);
            SendMessageW(
                window,
                WM_POWERBROADCAST,
                PBT_APMRESUMEAUTOMATIC as usize,
                0,
            );
            SendMessageW(window, WM_TIMER, 1, 0);
            wait(|| IsWindowEnabled(GetDlgItem(window, 113)) != 0);
            assert_eq!(
                responses.calls.load(Ordering::SeqCst),
                0,
                "Resume and reopen cannot start a check"
            );
            // Synthetic per-window DPI messages do not alter machine display settings.
            for dpi in [96usize, 192, 96] {
                let bounds = RECT {
                    left: 50,
                    top: 50,
                    right: 50 + 800 * dpi as i32 / 96,
                    bottom: 50 + 560 * dpi as i32 / 96,
                };
                SendMessageW(
                    window,
                    WM_DPICHANGED,
                    dpi | (dpi << 16),
                    &bounds as *const _ as isize,
                );
                let mut client = RECT::default();
                GetClientRect(window, &mut client);
                let rect = |id| {
                    let mut r = RECT::default();
                    assert_ne!(GetWindowRect(GetDlgItem(window, id), &mut r), 0);
                    r
                };
                let version = rect(163);
                let check = rect(160);
                let open = rect(161);
                let status = rect(162);
                let startup = rect(152);
                let editor = rect(105);
                let save = rect(110);
                assert!(version.right <= check.left && check.right <= open.left);
                assert!(status.bottom <= startup.top && editor.bottom <= save.top);
                let mut main = RECT::default();
                GetWindowRect(window, &mut main);
                assert!(open.right <= main.right && status.right <= main.right);
                assert!(editor.bottom > editor.top && client.bottom > 0);
            }
            let before = std::fs::read(&database).unwrap();
            responses.mode.store(3, Ordering::SeqCst);
            SendMessageW(window, WM_COMMAND, 160, 0);
            wait(|| text(GetDlgItem(window, 162)) == "Up to date.");
            assert_eq!(
                std::fs::read(&database).unwrap(),
                before,
                "Checking itself must not change database bytes"
            );
            responses.mode.store(0, Ordering::SeqCst);
            SendMessageW(window, WM_COMMAND, 160, 0);
            wait(|| responses.active.load(Ordering::SeqCst) == 1);
            let editor = GetDlgItem(window, 105);
            let draft: Vec<u16> = "owned unsaved draft"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            SendMessageW(editor, EM_SETSEL, 0, -1);
            SendMessageW(editor, EM_REPLACESEL, 1, draft.as_ptr() as isize);
            SendMessageW(window, WM_COMMAND, 151, 0);
            wait(|| IsWindowVisible(GetDlgItem(window, 153)) != 0);
            assert_eq!(
                responses.active.load(Ordering::SeqCst),
                1,
                "Exit prompt must not decide for the user"
            );
            assert_eq!(IsWindowEnabled(GetDlgItem(window, 160)), 0);
            SendMessageW(window, WM_COMMAND, 154, 0);
            assert_eq!(text(editor), "owned unsaved draft");
            assert_ne!(IsWindowEnabled(editor), 0);
            assert_eq!(responses.active.load(Ordering::SeqCst), 1);
            let began = Instant::now();
            exit(window);
            wait(|| IsWindow(window) == 0);
            assert!(
                began.elapsed() < Duration::from_secs(2),
                "Controlled cancellation should finish promptly"
            );
            assert_eq!(responses.active.load(Ordering::SeqCst), 0);
        });
        if IsWindow(window) != 0 {
            exit(window);
        }
        result
    });
    desktop::run_with_services(
        Registry,
        directory,
        Registration::default(),
        PathBuf::from("C:/Owned/app.exe"),
        true,
        UpdateChecker::new(replies),
        BrowserAdapter(browser),
    )
    .unwrap();
    if let Err(e) = driver.join().unwrap() {
        std::panic::resume_unwind(e)
    }
}
use crate::native_status;
use windows_sys::Win32::{
    Foundation::*,
    UI::{Controls::*, Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
};
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
