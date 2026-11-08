use environment_variable_manager::{
    startup::{RegistrationState, Startup, StartupRegistration},
    Application, RegistryReader, Scope, Variable,
};
use std::cell::RefCell;
struct Reader;
impl RegistryReader for Reader {
    fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
        Ok(vec![])
    }
}
#[derive(Default)]
struct Registration(RefCell<RegistrationState>);
impl StartupRegistration for Registration {
    fn state(&self) -> Result<RegistrationState, String> {
        Ok(self.0.borrow().clone())
    }
    fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        if s.command.as_deref() != expected {
            return Err("Registration changed".into());
        }
        s.command = desired.map(str::to_owned);
        Ok(())
    }
}
fn directory() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-startup-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).expect("Exclusively reserve owned disposable test data");
    path
}

#[test]
fn startup_defaults_once_persists_disable_and_respects_external_disable() {
    let dir = directory();
    let app = Application::open(&Reader, &dir);
    let reg = Registration::default();
    let mut startup =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(startup.enabled());
    assert_eq!(
        reg.state().unwrap().command.as_deref(),
        Some("\"C:/Owned/app.exe\" --tray")
    );
    startup.set_enabled(false, &reg).unwrap();
    assert!(
        !Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe"))
            .unwrap()
            .enabled()
    );
    startup.set_enabled(true, &reg).unwrap();
    reg.0.borrow_mut().command = None;
    assert!(
        !Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe"))
            .unwrap()
            .enabled()
    );
    assert!(reg.state().unwrap().command.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_move_repair_retains_ownership_for_enable_or_disable_retry() {
    struct FailOnce {
        inner: Registration,
        fail: std::cell::Cell<bool>,
    }
    impl StartupRegistration for FailOnce {
        fn state(&self) -> Result<RegistrationState, String> {
            self.inner.state()
        }
        fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
            if self.fail.replace(false) {
                return Err("Owned fixture transient write denial".into());
            }
            self.inner.replace(expected, desired)
        }
    }
    for enable in [false, true] {
        let dir = directory();
        let app = Application::open(&Reader, &dir);
        let reg = FailOnce {
            inner: Registration::default(),
            fail: std::cell::Cell::new(false),
        };
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Old/app.exe")).unwrap();
        reg.fail.set(true);
        let moved = std::path::Path::new("C:/Moved/app.exe");
        assert!(Startup::initialize(&app, &reg, moved).is_err());
        assert_eq!(
            reg.state().unwrap().command.as_deref(),
            Some("\"C:/Old/app.exe\" --tray")
        );
        let mut retry = Startup::initialize(&app, &reg, moved).unwrap();
        assert!(!retry.enabled());
        retry.set_enabled(enable, &reg).unwrap();
        assert_eq!(
            reg.state().unwrap().command.as_deref(),
            if enable {
                Some("\"C:/Moved/app.exe\" --tray")
            } else {
                None
            }
        );
        let reopened = Startup::initialize(&app, &reg, moved).unwrap();
        assert_eq!(reopened.enabled(), enable);
        // The recovery must not weaken protection for a truly foreign replacement.
        reg.inner.0.borrow_mut().command = Some("foreign registration".into());
        assert!(retry.set_enabled(true, &reg).is_err());
        assert!(retry.set_enabled(false, &reg).is_err());
        assert_eq!(
            reg.state().unwrap().command.as_deref(),
            Some("foreign registration")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn failed_settings_write_before_move_repair_preserves_in_memory_ownership() {
    let dir = directory();
    let app = Application::open(&Reader, &dir);
    let reg = Registration::default();
    Startup::initialize(&app, &reg, std::path::Path::new("C:/Old/app.exe")).unwrap();
    // A policy block defers automatic move repair until the explicit retry below.
    reg.0.borrow_mut().blocked = true;
    let mut moved =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Moved/app.exe")).unwrap();
    reg.0.borrow_mut().blocked = false;
    let database = dir.join("environment.sqlite");
    let original = std::fs::metadata(&database).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&database, read_only).unwrap();
    let failure = moved.set_enabled(true, &reg);
    std::fs::set_permissions(&database, original).unwrap();
    assert!(failure.is_err());
    assert_eq!(
        reg.state().unwrap().command.as_deref(),
        Some("\"C:/Old/app.exe\" --tray")
    );
    moved.set_enabled(true, &reg).unwrap();
    assert_eq!(
        reg.state().unwrap().command.as_deref(),
        Some("\"C:/Moved/app.exe\" --tray")
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn native_startup_changes_only_its_owned_value_and_refuses_stale_replacement() {
    use environment_variable_manager::startup::WindowsStartup;
    use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
    let path = format!(
        "Software\\EnvironmentManagerTests\\startup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut key = std::ptr::null_mut();
    let mut disposition = 0;
    unsafe {
        assert_eq!(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_ALL_ACCESS,
                std::ptr::null(),
                &mut key,
                &mut disposition
            ),
            ERROR_SUCCESS
        );
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        RegCloseKey(key);
    }
    let own = WindowsStartup::under_current_user(&path, "Owned");
    let other = WindowsStartup::under_current_user(&path, "Unrelated");
    other.replace(None, Some("leave exactly")).unwrap();
    own.replace(None, Some("\"C:\\Owned App\\app.exe\" --tray"))
        .unwrap();
    assert!(own.replace(None, None).is_err());
    assert_eq!(
        other.state().unwrap().command.as_deref(),
        Some("leave exactly")
    );
    own.replace(Some("\"C:\\Owned App\\app.exe\" --tray"), None)
        .unwrap();
    assert!(own.state().unwrap().command.is_none());
    assert_eq!(
        other.state().unwrap().command.as_deref(),
        Some("leave exactly")
    );
    other.replace(Some("leave exactly"), None).unwrap();
    unsafe {
        assert_eq!(
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(&path).as_ptr(),
                0,
                KEY_READ,
                &mut key
            ),
            ERROR_SUCCESS
        );
        let mut values = 0;
        assert_eq!(
            RegQueryInfoKeyW(
                key,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut values,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            ERROR_SUCCESS
        );
        assert_eq!(values, 0);
        RegCloseKey(key);
        assert_eq!(
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&path).as_ptr()),
            ERROR_SUCCESS
        );
    }
}
#[test]
fn startup_policy_unhealthy_initialization_and_foreign_entries_are_preserved() {
    let dir = directory();
    let mut app = Application::open(&Reader, &dir);
    let reg = Registration::default();
    app.set_monitoring_errors(vec![(Scope::User, "owned notification denial".into())]);
    assert!(Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).is_err());
    assert!(reg.state().unwrap().command.is_none());
    app.set_monitoring_errors(vec![]);
    reg.0.borrow_mut().blocked = true;
    let mut startup =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(!startup.enabled());
    assert!(startup.set_enabled(true, &reg).is_err());
    reg.0.borrow_mut().blocked = false;
    assert!(
        !Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe"))
            .unwrap()
            .enabled()
    );
    startup.set_enabled(true, &reg).unwrap();
    reg.0.borrow_mut().command = Some("foreign command".into());
    assert!(startup.set_enabled(false, &reg).is_err());
    assert_eq!(
        reg.state().unwrap().command.as_deref(),
        Some("foreign command")
    );
    assert!(
        !Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe"))
            .unwrap()
            .enabled()
    );
    assert_eq!(
        reg.state().unwrap().command.as_deref(),
        Some("foreign command")
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn successful_startup_registration_does_not_need_a_second_settings_write() {
    // The registry boundary removes settings write access immediately after registration.
    // A successful initial registration must already have durable enabled preference.
    struct LoseSettingsAfterRegistration {
        state: Registration,
        database: std::path::PathBuf,
    }
    impl StartupRegistration for LoseSettingsAfterRegistration {
        fn state(&self) -> Result<RegistrationState, String> {
            self.state.state()
        }
        fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
            self.state.replace(expected, desired)?;
            let mut perms = std::fs::metadata(&self.database).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&self.database, perms).unwrap();
            Ok(())
        }
    }
    let dir = directory();
    let app = Application::open(&Reader, &dir);
    let reg = LoseSettingsAfterRegistration {
        state: Registration::default(),
        database: dir.join("environment.sqlite"),
    };
    let result = Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe"));
    let mut perms = std::fs::metadata(&reg.database).unwrap().permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(&reg.database, perms).unwrap();
    assert!(
        result.is_ok(),
        "Startup should persist the enabled decision before registering"
    );
    let reopened =
        Startup::initialize(&app, &reg.state, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(reopened.enabled());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn temporary_startup_block_preserves_preference_and_unblock_recovers_without_enabling_again() {
    let dir = directory();
    let app = Application::open(&Reader, &dir);
    let reg = Registration::default();
    let startup =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(startup.enabled());
    let command = reg.state().unwrap().command;
    reg.0.borrow_mut().blocked = true;
    let mut blocked =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(!blocked.enabled());
    assert!(blocked.preferred_enabled());
    reg.0.borrow_mut().blocked = false;
    blocked.refresh_status(&reg).unwrap();
    assert!(blocked.enabled());
    assert!(
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe"))
            .unwrap()
            .enabled()
    );
    assert_eq!(reg.state().unwrap().command, command);
    blocked.set_enabled(false, &reg).unwrap();
    reg.0.borrow_mut().blocked = true;
    blocked.refresh_status(&reg).unwrap();
    reg.0.borrow_mut().blocked = false;
    blocked.refresh_status(&reg).unwrap();
    assert!(!blocked.enabled());
    assert!(!blocked.preferred_enabled());
    assert!(reg.state().unwrap().command.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn owned_windows_policy_fixture_ignores_run_ui_policy_and_recovers_temporary_startup_blocks() {
    use environment_variable_manager::startup::WindowsStartup;
    use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let path = format!(
        "Software\\EnvironmentManagerTests\\startup-policy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    eprintln!("Owned policy fixture: {path}");
    let reserve = |p: &str| {
        let mut key = std::ptr::null_mut();
        let mut disposition = 0;
        unsafe {
            assert_eq!(
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(p).as_ptr(),
                    0,
                    std::ptr::null(),
                    0,
                    KEY_ALL_ACCESS,
                    std::ptr::null(),
                    &mut key,
                    &mut disposition
                ),
                ERROR_SUCCESS
            )
        };
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        key
    };
    let root = reserve(&path);
    let policy = reserve(&format!("{path}\\Policies"));
    let approved = reserve(&format!("{path}\\StartupApproved"));
    let dword = |name: &str, value: u32| unsafe {
        assert_eq!(
            RegSetValueExW(
                policy,
                wide(name).as_ptr(),
                0,
                REG_DWORD,
                value.to_le_bytes().as_ptr(),
                4
            ),
            ERROR_SUCCESS
        )
    };
    let approval = |state: u8| {
        let mut value = [0u8; 12];
        value[0] = state;
        unsafe {
            assert_eq!(
                RegSetValueExW(
                    approved,
                    wide("Owned").as_ptr(),
                    0,
                    REG_BINARY,
                    value.as_ptr(),
                    12
                ),
                ERROR_SUCCESS
            )
        }
    };
    let dir = directory();
    let app = Application::open(&Reader, &dir);
    let reg = WindowsStartup::under_current_user(&path, "Owned");
    dword("NoRun", 1);
    let mut startup =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(startup.enabled(), "NoRun controls Run UI, not sign-in");
    let command = reg.state().unwrap().command;
    dword("DisableCurrentUserRun", 1);
    assert!(reg.state().unwrap().blocked);
    startup.refresh_status(&reg).unwrap();
    assert!(!startup.enabled());
    assert!(startup.preferred_enabled());
    let blocked =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(!blocked.enabled());
    assert!(blocked.preferred_enabled());
    dword("DisableCurrentUserRun", 0);
    startup.refresh_status(&reg).unwrap();
    assert!(startup.enabled());
    assert_eq!(reg.state().unwrap().command, command);
    approval(3);
    startup.refresh_status(&reg).unwrap();
    assert!(!startup.enabled());
    assert!(startup.preferred_enabled());
    let blocked =
        Startup::initialize(&app, &reg, std::path::Path::new("C:/Owned/app.exe")).unwrap();
    assert!(!blocked.enabled());
    assert!(blocked.preferred_enabled());
    approval(2);
    startup.refresh_status(&reg).unwrap();
    assert!(startup.enabled());
    assert_eq!(reg.state().unwrap().command, command);
    startup.set_enabled(false, &reg).unwrap();
    approval(3);
    startup.refresh_status(&reg).unwrap();
    approval(2);
    startup.refresh_status(&reg).unwrap();
    assert!(!startup.enabled());
    assert!(!startup.preferred_enabled());
    assert!(reg.state().unwrap().command.is_none());
    // Only our expected values/subkeys may remain before deleting the owned root.
    unsafe {
        for (key, expected_subkeys, expected_values) in
            [(root, 2, 0), (policy, 0, 2), (approved, 0, 1)]
        {
            let mut subkeys = 0;
            let mut values = 0;
            assert_eq!(
                RegQueryInfoKeyW(
                    key,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut subkeys,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut values,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                ERROR_SUCCESS
            );
            assert_eq!((subkeys, values), (expected_subkeys, expected_values));
        }
        for (name, expected) in [("NoRun", 1u32), ("DisableCurrentUserRun", 0u32)] {
            let mut kind = 0;
            let mut bytes = [0u8; 4];
            let mut size = 4;
            assert_eq!(
                RegQueryValueExW(
                    policy,
                    wide(name).as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    bytes.as_mut_ptr(),
                    &mut size
                ),
                ERROR_SUCCESS
            );
            assert_eq!((kind, size, bytes), (REG_DWORD, 4, expected.to_le_bytes()));
        }
        let mut kind = 0;
        let mut bytes = [0u8; 12];
        let mut size = 12;
        assert_eq!(
            RegQueryValueExW(
                approved,
                wide("Owned").as_ptr(),
                std::ptr::null(),
                &mut kind,
                bytes.as_mut_ptr(),
                &mut size
            ),
            ERROR_SUCCESS
        );
        let mut expected = [0u8; 12];
        expected[0] = 2;
        assert_eq!((kind, size, bytes), (REG_BINARY, 12, expected));
        RegCloseKey(approved);
        RegCloseKey(policy);
        RegCloseKey(root);
        assert_eq!(
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(&path).as_ptr()),
            ERROR_SUCCESS
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
