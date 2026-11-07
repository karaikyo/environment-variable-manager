use environment_variable_manager::{
    startup::{RegistrationState, Startup, StartupRegistration},
    Application, RegistryReader, Scope, Variable,
};
use rusqlite::{params, Connection};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

struct Reader(Vec<Variable>);
impl RegistryReader for Reader {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(self
            .0
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
fn directory() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-updates-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    eprintln!(
        "[fixture-owner] test={:?}; path={}",
        std::thread::current().name(),
        path.display()
    );
    std::fs::create_dir(&path)
        .expect("Exclusively reserve owned test directory; abort on collision");

    path
}
fn legacy(directory: &Path, pending: bool) -> Reader {
    let value = Variable::text(Scope::User, "Owned", " %ROOT%;a;;a;\n ", true);
    let opaque = Variable {
        scope: Scope::System,
        name: "Opaque".into(),
        registry_type: 7,
        bytes: vec![0, 255, 42],
    };
    let db = Connection::open(directory.join("environment.sqlite")).unwrap();
    db.execute_batch("PRAGMA application_id=1163283761; PRAGMA user_version=1;
CREATE TABLE baseline(scope INTEGER NOT NULL,name TEXT NOT NULL,registry_type INTEGER NOT NULL,raw BLOB NOT NULL,PRIMARY KEY(scope,name));
CREATE TABLE state(id INTEGER PRIMARY KEY,initialized INTEGER NOT NULL); INSERT INTO state VALUES(1,1);
CREATE TABLE operations(id INTEGER PRIMARY KEY,scope INTEGER NOT NULL,name TEXT NOT NULL,before_type INTEGER,before_raw BLOB,desired_type INTEGER,desired_raw BLOB,after_type INTEGER,after_raw BLOB,time_ms INTEGER NOT NULL,source TEXT NOT NULL,outcome TEXT NOT NULL);
CREATE TABLE settings(id INTEGER PRIMARY KEY,startup_enabled INTEGER NOT NULL,startup_command TEXT); INSERT INTO settings VALUES(1,0,NULL);").unwrap();
    for (scope, v) in [(0, &value), (1, &opaque)] {
        db.execute(
            "INSERT INTO baseline VALUES(?1,?2,?3,?4)",
            params![scope, v.name, v.registry_type, v.bytes],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO operations VALUES(11,0,'Owned',1,?1,2,?2,2,?2,1234,'app-made','applied')",
        params![
            Variable::text(Scope::User, "Owned", "before", false).bytes,
            value.bytes
        ],
    )
    .unwrap();
    if pending {
        db.execute("INSERT INTO operations VALUES(12,0,'Owned',2,?1,1,?2,NULL,NULL,1235,'app-made','pending')",params![value.bytes,Variable::text(Scope::User,"Owned","desired",false).bytes]).unwrap();
    }
    Reader(vec![value, opaque])
}
#[derive(Default)]
struct Registration(RefCell<RegistrationState>);
impl StartupRegistration for Registration {
    fn state(&self) -> Result<RegistrationState, String> {
        Ok(self.0.borrow().clone())
    }
    fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
        let mut state = self.0.borrow_mut();
        if state.command.as_deref() != expected {
            return Err("changed".into());
        }
        state.command = desired.map(str::to_owned);
        Ok(())
    }
}
#[test]
fn manual_update_migrates_populated_legacy_data_and_retains_verified_backup() {
    let dir = directory();
    let reader = legacy(&dir, true);
    let app = Application::open(&reader, &dir);
    assert!(
        dir.join("before-schema-1-to-2.sqlite").is_file(),
        "format change must retain a verified backup"
    );
    assert_eq!(app.baseline(), Some(reader.0.as_slice()));
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, 11);
    assert_eq!(history[0].time_ms, 1234);
    assert_eq!(history[0].after.as_ref(), Some(&reader.0[0]));
    let pending = app.pending_operations().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, 12);
    assert_eq!(pending[0].expected_before.as_ref(), Some(&reader.0[0]));
    assert_eq!(
        pending[0]
            .desired_after
            .as_ref()
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("desired")
    );
    assert!(app.storage_error().unwrap().contains("Pending"));
    let backup_bytes = std::fs::read(dir.join("before-schema-1-to-2.sqlite")).unwrap();
    let backup_dir = directory();
    std::fs::copy(
        dir.join("before-schema-1-to-2.sqlite"),
        backup_dir.join("environment.sqlite"),
    )
    .unwrap();
    let backup = Application::open(&reader, &backup_dir);
    assert_eq!(backup.history(None, 0, 100).unwrap(), history);
    assert_eq!(backup.pending_operations().unwrap(), pending);
    assert_eq!(backup.baseline(), app.baseline());
    assert_eq!(
        std::fs::read(dir.join("before-schema-1-to-2.sqlite")).unwrap(),
        backup_bytes
    );
    drop((app, backup));
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(backup_dir).unwrap();
}
#[test]
fn moved_folder_repairs_only_enabled_owned_unblocked_startup_and_preserves_preference() {
    for mode in ["enabled", "disabled", "blocked", "foreign", "removed"] {
        let dir = directory();
        let reader = legacy(&dir, false);
        let app = Application::open(&reader, &dir);
        let reg = Registration::default();
        let mut old = Startup::initialize(&app, &reg, Path::new("C:/Owned Old/app.exe")).unwrap();
        assert!(
            !old.preferred_enabled(),
            "legacy explicit disable survives migration"
        );
        old.set_enabled(true, &reg).unwrap();
        match mode {
            "disabled" => old.set_enabled(false, &reg).unwrap(),
            "blocked" => reg.0.borrow_mut().blocked = true,
            "foreign" => reg.0.borrow_mut().command = Some("foreign exact command".into()),
            "removed" => reg.0.borrow_mut().command = None,
            _ => {}
        }
        let previous = reg.state().unwrap().command;
        let moved = Startup::initialize(&app, &reg, Path::new("D:/Owned Moved/app.exe")).unwrap();
        if mode == "enabled" {
            assert_eq!(
                reg.state().unwrap().command.as_deref(),
                Some("\"D:/Owned Moved/app.exe\" --tray")
            );
            assert!(moved.enabled() && moved.preferred_enabled());
        } else {
            assert_eq!(
                reg.state().unwrap().command,
                previous,
                "disabled, blocked and foreign entries remain exact"
            );
            assert!(!moved.enabled());
            assert_eq!(moved.preferred_enabled(), mode == "blocked");
        }
        // Persisted ownership/preference survives another manual launch.
        let reopened =
            Startup::initialize(&app, &reg, Path::new("D:/Owned Moved/app.exe")).unwrap();
        assert_eq!(reopened.enabled(), moved.enabled());
        assert_eq!(reopened.preferred_enabled(), moved.preferred_enabled());
        drop((app, old, moved, reopened));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn failed_migration_preserves_original_and_retry_never_overwrites_prior_backups() {
    let dir = directory();
    let reader = legacy(&dir, false);
    for name in [
        "before-operations.sqlite",
        "before-settings.sqlite",
        "before-schema-1-to-2.sqlite",
    ] {
        std::fs::write(dir.join(name), b"owned prior backup bytes").unwrap();
    }
    // An incompatible old extension fails after the transaction adds operations.
    // SQL is fixture fault injection only; observable verification uses Application.
    let db = Connection::open(dir.join("environment.sqlite")).unwrap();
    db.execute_batch(
        "DROP TABLE operations; DROP TABLE settings; CREATE TABLE settings(incompatible TEXT)",
    )
    .unwrap();
    drop(db);
    let original = std::fs::read(dir.join("environment.sqlite")).unwrap();
    let mut failed = Application::open(&reader, &dir);
    assert!(failed.storage_error().is_some());
    assert_eq!(
        failed.browse("", None).len(),
        2,
        "Windows remains readable during migration failure"
    );
    assert_eq!(
        std::fs::read(dir.join("environment.sqlite")).unwrap(),
        original
    );
    let first_backup = std::fs::read(dir.join("before-schema-1-to-2-1.sqlite")).unwrap();
    failed.refresh(&reader);
    assert!(failed.storage_error().is_some());
    assert_eq!(
        std::fs::read(dir.join("environment.sqlite")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(dir.join("before-schema-1-to-2-1.sqlite")).unwrap(),
        first_backup
    );
    assert!(dir.join("before-schema-1-to-2-2.sqlite").exists());
    drop(failed);
    // Repair only the deliberate fixture fault, then retry the very same database.
    let db = Connection::open(dir.join("environment.sqlite")).unwrap();
    db.execute_batch("DROP TABLE settings").unwrap();
    drop(db);
    let app = Application::open(&reader, &dir);
    assert_eq!(app.storage_error(), None);
    assert_eq!(app.baseline(), Some(reader.0.as_slice()));
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    assert!(app.pending_operations().unwrap().is_empty());
    assert!(dir.join("before-schema-1-to-2-3.sqlite").exists());
    for name in [
        "before-operations.sqlite",
        "before-settings.sqlite",
        "before-schema-1-to-2.sqlite",
    ] {
        assert_eq!(
            std::fs::read(dir.join(name)).unwrap(),
            b"owned prior backup bytes"
        );
    }
    assert_eq!(
        std::fs::read(dir.join("before-schema-1-to-2-1.sqlite")).unwrap(),
        first_backup
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unsupported_newer_schema_refuses_all_unsafe_use_without_replacing_data() {
    let dir = directory();
    let reader = legacy(&dir, false);
    let db = Connection::open(dir.join("environment.sqlite")).unwrap();
    db.execute_batch("PRAGMA user_version=99").unwrap();
    drop(db);
    let original = std::fs::read(dir.join("environment.sqlite")).unwrap();
    let mut app = Application::open(&reader, &dir);
    assert!(app.storage_error().unwrap().contains("newer schema"));
    assert!(app.history(None, 0, 100).is_err());
    assert!(app.clear_history(None).is_err());
    assert!(app.pending_operations().is_err());
    assert!(app.backup_history(&dir.join("not-created.sqlite")).is_err());
    assert!(!dir.join("not-created.sqlite").exists());
    app.edit_entries(Scope::User, "Owned", vec!["changed".into()])
        .unwrap();
    assert!(!app.can_save(Scope::User, "Owned"));
    app.refresh(&reader);
    assert!(app.storage_error().is_some());
    assert_eq!(
        std::fs::read(dir.join("environment.sqlite")).unwrap(),
        original
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn clearing_live_history_retains_migration_backup_and_pending_information() {
    let dir = directory();
    let reader = legacy(&dir, true);
    let app = Application::open(&reader, &dir);
    let pending = app.pending_operations().unwrap();
    let baseline = app.baseline().unwrap().to_vec();
    let backup_path = dir.join("before-schema-1-to-2.sqlite");
    let bytes = std::fs::read(&backup_path).unwrap();
    assert_eq!(app.clear_history(None).unwrap(), 1);
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    assert_eq!(app.pending_operations().unwrap(), pending);
    assert_eq!(app.baseline(), Some(baseline.as_slice()));
    assert_eq!(std::fs::read(&backup_path).unwrap(), bytes);
    drop(app);
    let copy = directory();
    std::fs::copy(backup_path, copy.join("environment.sqlite")).unwrap();
    let retained = Application::open(&reader, &copy);
    assert_eq!(retained.history(None, 0, 100).unwrap().len(), 1);
    assert_eq!(retained.pending_operations().unwrap(), pending);
    drop(retained);
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(copy).unwrap();
}
#[test]
fn actual_same_computer_folder_move_repairs_only_owned_native_registration() {
    use environment_variable_manager::startup::WindowsStartup;
    use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
    let dir = directory();
    let old = dir.join("Old App Folder");
    let moved = dir.join("Moved App Folder");
    std::fs::create_dir(&old).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), old.join("app.exe")).unwrap();
    let key_path = format!(
        "Software\\EnvironmentManagerTests\\updates-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let wide: Vec<u16> = key_path.encode_utf16().chain(Some(0)).collect();
    let mut key = std::ptr::null_mut();
    let mut disposition = 0;
    unsafe {
        assert_eq!(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide.as_ptr(),
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
        assert_eq!(
            disposition, REG_CREATED_NEW_KEY,
            "exclusive owned registration fixture"
        );
        RegCloseKey(key);
    }
    let reg = WindowsStartup::under_current_user(&key_path, "Owned");
    let unrelated = WindowsStartup::under_current_user(&key_path, "Unrelated");
    unrelated
        .replace(None, Some("leave exact unrelated bytes"))
        .unwrap();
    let app = Application::open(&Reader(vec![]), &old.join("data"));
    let startup = Startup::initialize(&app, &reg, &old.join("app.exe")).unwrap();
    assert!(startup.enabled());
    drop((app, startup));
    std::fs::rename(&old, &moved).unwrap();
    let app = Application::open(&Reader(vec![]), &moved.join("data"));
    let mut startup = Startup::initialize(&app, &reg, &moved.join("app.exe")).unwrap();
    assert!(startup.enabled() && startup.preferred_enabled());
    assert_eq!(
        reg.state().unwrap().command,
        Some(format!("\"{}\" --tray", moved.join("app.exe").display()))
    );
    assert_eq!(
        unrelated.state().unwrap().command.as_deref(),
        Some("leave exact unrelated bytes")
    );
    startup.set_enabled(false, &reg).unwrap();
    unrelated
        .replace(Some("leave exact unrelated bytes"), None)
        .unwrap();
    unsafe {
        assert_eq!(
            RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_READ, &mut key),
            ERROR_SUCCESS
        );
        let mut values = 0;
        let mut children = 0;
        assert_eq!(
            RegQueryInfoKeyW(
                key,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut children,
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
        assert_eq!(
            (children, values),
            (0, 0),
            "verify exact owned root before cleanup"
        );
        RegCloseKey(key);
        assert_eq!(
            RegDeleteKeyW(HKEY_CURRENT_USER, wide.as_ptr()),
            ERROR_SUCCESS
        );
    }
    drop((app, startup));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn migration_refuses_unexpected_journaling_without_changing_original_format() {
    let dir = directory();
    let reader = legacy(&dir, false);
    let db = Connection::open(dir.join("environment.sqlite")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    drop(db);
    let original = std::fs::read(dir.join("environment.sqlite")).unwrap();
    let app = Application::open(&reader, &dir);
    assert!(
        app.storage_error().is_some(),
        "an unexpected journal mode must fail closed before migration writes"
    );
    assert_eq!(
        std::fs::read(dir.join("environment.sqlite")).unwrap(),
        original
    );
    assert!(!dir.join("before-schema-1-to-2.sqlite").exists());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
