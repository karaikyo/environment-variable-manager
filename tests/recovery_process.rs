//! Process-boundary proof using only a newly reserved HKCU fixture subtree.
use environment_variable_manager::{
    windows_registry::WindowsRegistry, Application, RegistryReader, RegistryWriter, Scope, Variable,
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    process::Command,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
struct Boundary {
    registry: WindowsRegistry,
    boundary: String,
    directory: PathBuf,
    wrote: Cell<bool>,
}
impl Boundary {
    fn pause(&self) -> ! {
        std::fs::write(self.directory.join("boundary-ready"), b"ready").unwrap();
        loop {
            std::thread::park();
        }
    }
    fn before(&self) {
        if self.boundary == "before-write" {
            self.pause()
        }
    }
    fn after(&self) {
        self.wrote.set(true);
        if self.boundary == "after-write" {
            self.pause()
        }
    }
}
impl RegistryReader for Boundary {
    fn read_scope(&self, s: Scope) -> Result<Vec<Variable>, String> {
        let values = self.registry.read_scope(s)?;
        if self.wrote.get() && self.boundary == "before-result" {
            self.pause()
        }
        Ok(values)
    }
}
impl RegistryWriter for Boundary {
    fn write_user(&self, v: &Variable) -> Result<(), String> {
        self.before();
        self.registry.write_user(v)?;
        self.after();
        Ok(())
    }
    fn delete_user(&self, n: &str) -> Result<(), String> {
        self.before();
        self.registry.delete_user(n)?;
        self.after();
        Ok(())
    }
}
#[test]
#[ignore = "launched only by the owned-fixture parent"]
fn recovery_boundary_child() {
    let path = std::env::var("EVM_RECOVERY_TEST_KEY").unwrap();
    assert!(path.starts_with("Software\\EnvironmentManagerTests\\recovery-"));
    let directory = PathBuf::from(std::env::var_os("EVM_RECOVERY_TEST_DATA").unwrap());
    let operation = std::env::var("EVM_RECOVERY_TEST_OPERATION").unwrap();
    let boundary = Boundary {
        registry: WindowsRegistry::under_current_user(&path, &format!("{path}\\System")),
        boundary: std::env::var("EVM_RECOVERY_TEST_BOUNDARY").unwrap(),
        directory: directory.clone(),
        wrote: Cell::new(false),
    };
    let mut app = Application::open(&boundary, &directory);
    assert_eq!(app.storage_error(), None);
    match operation.as_str() {
        "create" => {
            app.create_user("Owned").unwrap();
            app.edit_entries(Scope::User, "Owned", vec!["desired".into()])
                .unwrap();
            app.save(&boundary, Scope::User, "Owned").unwrap();
        }
        "edit" => {
            app.edit_entries(Scope::User, "Owned", vec!["desired".into()])
                .unwrap();
            app.save(&boundary, Scope::User, "Owned").unwrap();
        }
        "delete" => {
            app.request_delete(Scope::User, "Owned").unwrap();
            app.confirm_delete(&boundary).unwrap();
        }
        _ => panic!("invalid fixture operation"),
    }
    panic!("Boundary was not reached")
}
fn wait_ready(child: &mut std::process::Child, directory: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.join("boundary-ready").exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "Child exited before boundary"
        );
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("Child did not reach boundary");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn forced_termination_reconciles_create_edit_delete_and_preserves_later_external_values() {
    for operation in ["create", "edit", "delete"] {
        for boundary in ["before-write", "after-write", "before-result"] {
            for external in [false, true] {
                let suffix = format!(
                    "recovery-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                );
                let key_path = format!("Software\\EnvironmentManagerTests\\{suffix}");
                let mut key = null_mut();
                let mut disposition = 0;
                unsafe {
                    assert_eq!(
                        RegCreateKeyExW(
                            HKEY_CURRENT_USER,
                            wide(&key_path).as_ptr(),
                            0,
                            null(),
                            0,
                            KEY_READ | KEY_WRITE,
                            null(),
                            &mut key,
                            &mut disposition
                        ),
                        ERROR_SUCCESS
                    );
                    assert_eq!(
                        disposition, REG_CREATED_NEW_KEY,
                        "Abort on fixture collision"
                    );
                    RegCloseKey(key);
                }
                let registry =
                    WindowsRegistry::under_current_user(&key_path, &format!("{key_path}\\System"));
                assert!(registry.read_scope(Scope::User).unwrap().is_empty());
                let before = if operation == "create" {
                    None
                } else {
                    Some(Variable::text(Scope::User, "Owned", "before", false))
                };
                if let Some(value) = &before {
                    registry.write_user(value).unwrap();
                }
                let desired = if operation == "delete" {
                    None
                } else {
                    Some(Variable::text(Scope::User, "Owned", "desired", false))
                };
                let directory = std::env::temp_dir().join(&suffix);
                assert!(!directory.exists());
                drop(Application::open(&registry, &directory));
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--ignored",
                        "--exact",
                        "recovery_boundary_child",
                        "--nocapture",
                    ])
                    .env("EVM_RECOVERY_TEST_KEY", &key_path)
                    .env("EVM_RECOVERY_TEST_DATA", &directory)
                    .env("EVM_RECOVERY_TEST_OPERATION", operation)
                    .env("EVM_RECOVERY_TEST_BOUNDARY", boundary)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                wait_ready(&mut child, &directory);
                child.kill().unwrap();
                assert!(!child.wait().unwrap().success());
                let at_boundary = if boundary == "before-write" {
                    before.clone()
                } else {
                    desired.clone()
                };
                assert_eq!(
                    registry.read_scope(Scope::User).unwrap(),
                    at_boundary.clone().into_iter().collect::<Vec<_>>()
                );
                let current = if external {
                    // This writer owns the fixture and checks the exact expected state first.
                    let value = Variable::text(Scope::User, "Owned", "later external", false);
                    registry.write_user(&value).unwrap();
                    Some(value)
                } else {
                    at_boundary
                };
                let mut app = Application::open(&registry, &directory);
                let pending = app.pending_operations().unwrap();
                assert_eq!(pending.len(), 1);
                assert_eq!(pending[0].expected_before, before);
                assert_eq!(pending[0].desired_after, desired);
                assert!(app.storage_error().is_some());
                // A live backup must retain unresolved intents as data too.
                let backup_directory = directory.join("backup");
                std::fs::create_dir(&backup_directory).unwrap();
                app.backup_history(&backup_directory.join("environment.sqlite"))
                    .unwrap();
                assert_eq!(
                    Application::open(&registry, &backup_directory)
                        .pending_operations()
                        .unwrap(),
                    pending
                );
                app.refresh(&registry);
                assert_eq!(app.storage_error(), None);
                let history = app.history(None, 0, 100).unwrap();
                assert_eq!(history.len(), 1);
                assert_eq!(
                    history[0].outcome,
                    if external {
                        "uncertain"
                    } else if boundary == "before-write" {
                        "not-applied"
                    } else {
                        "applied"
                    }
                );
                assert!(history[0].source.contains("recovery gap"));
                assert_eq!(history[0].after, current);
                assert!(app.pending_operations().unwrap().is_empty());
                app.observe(&registry);
                app.refresh(&registry);
                assert_eq!(app.history(None, 0, 100).unwrap(), history);
                assert_eq!(
                    registry.read_scope(Scope::User).unwrap(),
                    current.into_iter().collect::<Vec<_>>(),
                    "Never replay or roll back stale intent"
                );
                assert!(registry.read_scope(Scope::System).unwrap().is_empty());
                // Only this exclusively reserved, fully verified fixture subtree is removed.
                unsafe {
                    assert_eq!(
                        RegDeleteTreeW(HKEY_CURRENT_USER, wide(&key_path).as_ptr()),
                        ERROR_SUCCESS
                    );
                }
                std::fs::remove_dir_all(directory).unwrap();
            }
        }
    }
}
