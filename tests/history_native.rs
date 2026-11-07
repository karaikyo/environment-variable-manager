//! Real history-window commands, isolated so a native callback abort is observable.
use environment_variable_manager::{history_window, Application, RegistryReader, Scope, Variable};
use std::{
    path::PathBuf,
    process::Command,
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::{Controls::*, WindowsAndMessaging::*};

struct Snapshot;
impl RegistryReader for Snapshot {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(if scope == Scope::User {
            vec![Variable::text(scope, "OwnedFixture", "original", false)]
        } else {
            vec![]
        })
    }
}
fn run_child(test: &str) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "evm-history-native-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).expect("reserve new disposable directory");
    let log = std::fs::File::create(directory.join("child.log")).unwrap();
    assert!(std::env::var_os("EVM_HISTORY_NATIVE_DATA").is_none());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture"])
        .env("EVM_HISTORY_NATIVE_DATA", &directory)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "Owned native child timed out; evidence: {}",
                directory.display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = std::fs::read_to_string(directory.join("child.log")).unwrap();
    assert!(
        status.success(),
        "Native history child failed {status}; evidence: {}\n{output}",
        directory.display()
    );
    let canonical = directory.canonicalize().unwrap();
    assert!(canonical.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    std::fs::remove_dir_all(canonical).unwrap();
}
unsafe fn pump() {
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn history_close_and_system_close_leave_drafts_alive_and_allow_reopen() {
    run_child("native_history_close_child");
}
#[test]
#[ignore = "owned child process invoked by the native lifecycle parent"]
fn native_history_close_child() {
    let directory =
        PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap()).join("data");
    let mut app = Application::open(&Snapshot, &directory);
    app.edit_entries(Scope::User, "OwnedFixture", vec!["unsaved".into()])
        .unwrap();
    unsafe {
        assert_ne!(
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES
            }),
            0
        );
        for system_close in [false, true, false] {
            let window = history_window::open(null_mut(), directory.clone(), None).unwrap();
            assert_ne!(IsWindow(window), 0);
            assert!(
                !native_status::history_loaded(window),
                "An HWND is not a completed history page"
            );
            pump();
            SendMessageW(
                window,
                if system_close {
                    WM_SYSCOMMAND
                } else {
                    WM_CLOSE
                },
                if system_close { SC_CLOSE as usize } else { 0 },
                0,
            );
            assert_eq!(
                IsWindow(window),
                0,
                "normal history close must destroy only history"
            );
            pump();
            assert!(app.has_unsaved_edits());
            assert!(app.history(None, 0, 50).unwrap().is_empty());
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
struct ForeignWindow {
    child: std::process::Child,
    window: windows_sys::Win32::Foundation::HWND,
}
impl ForeignWindow {
    fn start() -> Self {
        let directory = PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_history_foreign_window_child",
                "--ignored",
                "--nocapture",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let ready = directory.join("foreign-window");
        while !ready.exists() {
            assert!(child.try_wait().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let window = std::fs::read_to_string(ready)
            .unwrap()
            .parse::<usize>()
            .unwrap() as _;
        Self { child, window }
    }
}
impl Drop for ForeignWindow {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
        }
        self.child.wait().unwrap();
    }
}
#[test]
fn history_open_ignores_another_process_window_with_the_same_class() {
    run_child("native_history_collision_child");
}
#[test]
fn history_close_all_never_waits_for_another_process_window() {
    run_child("native_history_foreign_close_child");
}
#[test]
#[ignore = "owned child process invoked by the native lifecycle parent"]
fn native_history_collision_child() {
    let foreign = ForeignWindow::start();
    let directory =
        PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap()).join("data");
    let _app = Application::open(&Snapshot, &directory);
    unsafe {
        assert_ne!(
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES
            }),
            0
        );
        let own = history_window::open(null_mut(), directory, None).unwrap();
        assert_ne!(own, foreign.window, "history must belong to this process");
        let mut owner = 0;
        GetWindowThreadProcessId(own, &mut owner);
        assert_eq!(owner, std::process::id());
        history_window::close_all();
        assert_eq!(IsWindow(own), 0);
        assert_ne!(
            IsWindow(foreign.window),
            0,
            "foreign fixture stays untouched"
        );
    }
}
#[test]
#[ignore = "owned child process invoked by the native lifecycle parent"]
fn native_history_foreign_close_child() {
    let foreign = ForeignWindow::start();
    let started = Instant::now();
    unsafe {
        history_window::close_all();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "foreign windows must not delay close"
        );
        assert_ne!(IsWindow(foreign.window), 0);
    }
}
#[test]
#[ignore = "owned fake window process invoked only by collision fixtures"]
fn native_history_foreign_window_child() {
    use windows_sys::Win32::{Foundation::*, System::LibraryLoader::GetModuleHandleW};
    unsafe extern "system" fn fake_proc(
        window: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> LRESULT {
        match message {
            WM_TIMER => {
                DestroyWindow(window);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(window, message, w, l),
        }
    }
    unsafe {
        let class = wide("EnvironmentManager.PagedHistory");
        let instance = GetModuleHandleW(std::ptr::null());
        assert_ne!(
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(fake_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            }),
            0
        );
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Owned collision fixture").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            100,
            100,
            null_mut(),
            null_mut(),
            instance,
            std::ptr::null(),
        );
        assert!(!window.is_null());
        SetTimer(window, 1, 4000, None);
        let directory = PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap());
        std::fs::write(
            directory.join("foreign-window"),
            format!("{}", window as usize),
        )
        .unwrap();
        let mut message = MSG::default();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn wait_native(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "native fixture condition timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
unsafe fn owned_window(class: &str) -> windows_sys::Win32::Foundation::HWND {
    native_status::find_owned_window(class, std::process::id())
}
struct NoStartup;
impl environment_variable_manager::startup::StartupRegistration for NoStartup {
    fn state(&self) -> Result<environment_variable_manager::startup::RegistrationState, String> {
        Ok(environment_variable_manager::startup::RegistrationState {
            command: None,
            blocked: true,
        })
    }
    fn replace(&self, _: Option<&str>, _: Option<&str>) -> Result<(), String> {
        panic!("native test never changes startup")
    }
}
impl environment_variable_manager::RegistryWriter for Snapshot {
    fn write_user(&self, _: &Variable) -> Result<(), String> {
        panic!("native test never writes Windows")
    }
}
#[test]
fn native_exit_waits_for_inflight_history_clear() {
    run_child("native_history_clear_exit_child");
}
#[test]
#[ignore = "owned child process invoked by the native lifecycle parent"]
fn native_history_clear_exit_child() {
    let directory =
        PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap()).join("data");
    struct Empty;
    impl RegistryReader for Empty {
        fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
            Ok(vec![])
        }
    }
    let mut seed = Application::open(&Snapshot, &directory);
    seed.observe(&Empty);
    assert_eq!(seed.history(None, 0, 50).unwrap().len(), 1);
    let driver_data = directory.clone();
    let driver = std::thread::spawn(move || unsafe {
        let mut main = null_mut();
        wait_native(|| {
            main = owned_window("EnvironmentManager.Main");
            if main.is_null() {
                return false;
            }
            let mut title = [0u16; 128];
            let length = GetWindowTextW(main, title.as_mut_ptr(), 128);
            String::from_utf16_lossy(&title[..length as usize]) == "Environment Manager"
        });
        PostMessageW(main, WM_COMMAND, 112, 0);
        let mut history = null_mut();
        wait_native(|| {
            history = owned_window("EnvironmentManager.PagedHistory");
            !history.is_null()
                && SendMessageW(GetDlgItem(history, 202), LVM_GETITEMCOUNT, 0, 0) > 0
                && windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(GetDlgItem(
                    history, 206,
                )) != 0
        });
        let lock = rusqlite::Connection::open(driver_data.join("environment.sqlite")).unwrap();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        PostMessageW(history, WM_COMMAND, 206, 0);
        PostMessageW(history, WM_COMMAND, 207, 0);
        wait_native(|| {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(GetDlgItem(
                history, 206,
            )) == 0
        });
        PostMessageW(
            main,
            WM_COMMAND,
            environment_variable_manager::native_lifecycle::EXIT,
            0,
        );
        std::thread::sleep(Duration::from_millis(100));
        assert_ne!(
            IsWindow(main),
            0,
            "normal Exit must await the accepted history clear"
        );
        lock.execute_batch("ROLLBACK").unwrap();
        wait_native(|| IsWindow(main) == 0);
    });
    environment_variable_manager::desktop::run_with_startup(
        Snapshot,
        directory.clone(),
        NoStartup,
        PathBuf::from("C:/Owned/app.exe"),
        false,
    )
    .unwrap();
    driver.join().unwrap();
    assert!(
        seed.history(None, 0, 50).unwrap().is_empty(),
        "clear completed before normal app exit"
    );
}

#[path = "../examples/support/native_status.rs"]
mod native_status;
#[test]
fn native_readiness_rejects_saved_baseline_when_a_scope_is_unreadable() {
    run_child("native_unreadable_readiness_child");
}
#[test]
#[ignore = "owned child process invoked by the native lifecycle parent"]
fn native_unreadable_readiness_child() {
    let directory =
        PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap()).join("data");
    let seed = Application::open(&Snapshot, &directory);
    assert!(seed.baseline().is_some());
    struct Unreadable;
    impl RegistryReader for Unreadable {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            if scope == Scope::System {
                Err("Owned simulated read denial".into())
            } else {
                Snapshot.read_scope(scope)
            }
        }
    }
    impl environment_variable_manager::RegistryWriter for Unreadable {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("No Windows writes")
        }
    }
    let driver = std::thread::spawn(move || unsafe {
        let mut main = null_mut();
        wait_native(|| {
            main = owned_window("EnvironmentManager.Main");
            let mut title = [0u16; 128];
            let length = GetWindowTextW(main, title.as_mut_ptr(), 128);
            String::from_utf16_lossy(&title[..length as usize]) == "Environment Manager"
        });
        // Navigation refreshes the ordinary status after startup diagnostics.
        SetWindowTextW(GetDlgItem(main, 101), wide("Owned").as_ptr());
        let healthy = native_status::healthy(main);
        let observing = native_status::observing(main);
        PostMessageW(
            main,
            WM_COMMAND,
            environment_variable_manager::native_lifecycle::EXIT,
            0,
        );
        assert!(
            !healthy && !observing,
            "a saved baseline does not prove both scopes are readable"
        );
    });
    environment_variable_manager::desktop::run_with_startup(
        Unreadable,
        directory,
        NoStartup,
        PathBuf::from("C:/Owned/app.exe"),
        false,
    )
    .unwrap();
    driver.join().unwrap();
}

#[test]
fn native_sampling_accepts_healthy_refresh_without_claiming_ui_readiness() {
    run_child("native_busy_refresh_sampling_child");
}
#[test]
#[ignore = "owned child process invoked by the native lifecycle parent"]
fn native_busy_refresh_sampling_child() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct SlowSnapshot(Arc<AtomicBool>);
    impl RegistryReader for SlowSnapshot {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            if self.0.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
            Snapshot.read_scope(scope)
        }
    }
    impl environment_variable_manager::RegistryWriter for SlowSnapshot {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("No Windows writes")
        }
    }
    let directory =
        PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap()).join("data");
    let slow = Arc::new(AtomicBool::new(false));
    let driver_slow = slow.clone();
    let driver = std::thread::spawn(move || unsafe {
        let mut main = null_mut();
        wait_native(|| {
            main = owned_window("EnvironmentManager.Main");
            !main.is_null() && native_status::healthy(main)
        });
        driver_slow.store(true, Ordering::SeqCst);
        SendMessageW(main, WM_COMMAND, 113, 0);
        let during = native_status::inspect(main);
        let observing = native_status::observing(main);
        driver_slow.store(false, Ordering::SeqCst);
        wait_native(|| native_status::healthy(main));
        SendMessageW(main, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
        let suspended = native_status::inspect(main);
        PostMessageW(
            main,
            WM_COMMAND,
            environment_variable_manager::native_lifecycle::EXIT,
            0,
        );
        assert!(
            !suspended.observing() && suspended.monitoring_error,
            "A reported monitoring gap must fail sampling: {suspended:?}"
        );
        assert!(
            !during.refresh_enabled && during.exact_healthy_status,
            "Expected ordinary in-flight refresh: {during:?}"
        );
        assert!(
            observing,
            "A healthy ongoing refresh is not recording interruption: {during:?}"
        );
    });
    environment_variable_manager::desktop::run_with_startup(
        SlowSnapshot(slow),
        directory,
        NoStartup,
        PathBuf::from("C:/Owned/app.exe"),
        false,
    )
    .unwrap();
    driver.join().unwrap();
}

#[test]
fn native_path_previews_preserve_backslashes_when_loading_editing_and_viewing_history() {
    run_child("native_path_preview_child");
}
#[test]
#[ignore = "owned child process invoked by the native display parent"]
fn native_path_preview_child() {
    const VALUE: &str =
        "C:\\Program Files\\Tool;\\\\server\\share\\bin;C:\\new\\tools;line\r\nbreak;";
    struct Paths(&'static str);
    impl RegistryReader for Paths {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            Ok(if scope == Scope::User {
                vec![Variable::text(scope, "OwnedPaths", self.0, false)]
            } else {
                vec![]
            })
        }
    }
    impl environment_variable_manager::RegistryWriter for Paths {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("Display test never writes Windows");
        }
    }
    unsafe fn cell(
        window: windows_sys::Win32::Foundation::HWND,
        row: usize,
        column: i32,
    ) -> String {
        let mut text = [0u16; 512];
        let mut item = LVITEMW {
            iSubItem: column,
            pszText: text.as_mut_ptr(),
            cchTextMax: text.len() as i32,
            ..Default::default()
        };
        let length = SendMessageW(window, LVM_GETITEMTEXTW, row, &mut item as *mut _ as isize);
        String::from_utf16_lossy(&text[..length as usize])
    }
    unsafe fn text(window: windows_sys::Win32::Foundation::HWND) -> String {
        let mut buffer = [0u16; 512];
        let length = GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32);
        String::from_utf16_lossy(&buffer[..length as usize])
    }
    let directory =
        PathBuf::from(std::env::var_os("EVM_HISTORY_NATIVE_DATA").unwrap()).join("data");
    let mut seed = Application::open(&Paths(r"C:\Old"), &directory);
    seed.observe(&Paths(VALUE));
    let driver = std::thread::spawn(move || unsafe {
        let mut main = null_mut();
        wait_native(|| {
            main = native_status::find_owned_window("EnvironmentManager.Main", std::process::id());
            !main.is_null() && native_status::healthy(main)
        });
        struct ExitOnDrop(windows_sys::Win32::Foundation::HWND);
        impl Drop for ExitOnDrop {
            fn drop(&mut self) {
                unsafe {
                    PostMessageW(
                        self.0,
                        WM_COMMAND,
                        environment_variable_manager::native_lifecycle::EXIT,
                        0,
                    );
                    PostMessageW(self.0, WM_COMMAND, 153, 0);
                }
            }
        }
        let _exit = ExitOnDrop(main);
        let entries = GetDlgItem(main, 104);
        let expected = [
            r"C:\Program Files\Tool",
            r"\\server\share\bin",
            r"C:\new\tools",
            r"line\r\nbreak",
            "(empty entry)",
        ];
        for (row, value) in expected.iter().enumerate() {
            assert_eq!(cell(entries, row, 1), *value, "loaded entry row {row}");
        }
        assert_eq!(text(GetDlgItem(main, 105)), expected[0]);
        SendMessageW(GetDlgItem(main, 105), EM_SETSEL, 0, -1);
        SendMessageW(
            GetDlgItem(main, 105),
            EM_REPLACESEL,
            1,
            wide(r"D:\Changed\bin").as_ptr() as isize,
        );
        assert_eq!(
            cell(entries, 0, 1),
            r"D:\Changed\bin",
            "edited entry preview"
        );
        SendMessageW(main, WM_COMMAND, 111, 0);
        wait_native(|| {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(GetDlgItem(main, 113))
                != 0
                && text(GetDlgItem(main, 105)) == expected[0]
        });
        assert_eq!(
            cell(entries, 0, 1),
            expected[0],
            "discard reloads the raw path"
        );
        SendMessageW(main, WM_COMMAND, 112, 0);
        let mut history = null_mut();
        wait_native(|| {
            history = native_status::find_owned_window(
                "EnvironmentManager.PagedHistory",
                std::process::id(),
            );
            native_status::history_loaded(history)
        });
        let rows = GetDlgItem(history, 203);
        for (row, value) in expected[..3].iter().enumerate() {
            assert_eq!(
                cell(rows, row, 5),
                format!("\"{value}\""),
                "history After row {row}"
            );
        }
        SendMessageW(history, WM_CLOSE, 0, 0);
        PostMessageW(
            main,
            WM_COMMAND,
            environment_variable_manager::native_lifecycle::EXIT,
            0,
        );
    });
    environment_variable_manager::desktop::run_with_startup(
        Paths(VALUE),
        directory,
        NoStartup,
        PathBuf::from("C:/Owned/app.exe"),
        false,
    )
    .unwrap();
    driver.join().unwrap();
    assert_eq!(
        seed.history(None, 0, 50).unwrap()[0]
            .after
            .as_ref()
            .unwrap()
            .raw_text()
            .as_deref(),
        Some(VALUE)
    );
}
