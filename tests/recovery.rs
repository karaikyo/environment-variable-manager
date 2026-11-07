use environment_variable_manager::{Application, RegistryReader, RegistryWriter, Scope, Variable};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
};
struct Registry {
    values: RefCell<Vec<Variable>>,
    fail_recheck: Cell<u8>,
}
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        let countdown = self.fail_recheck.get();
        if countdown > 0 {
            self.fail_recheck.set(countdown - 1);
            if countdown == 1 {
                return Err("temporary read denial".into());
            }
        }
        Ok(self
            .values
            .borrow()
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
impl RegistryWriter for Registry {
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        let mut values = self.values.borrow_mut();
        values.retain(|v| !environment_variable_manager::same_name(&v.name, &value.name));
        values.push(value.clone());
        Ok(())
    }
    fn delete_user(&self, name: &str) -> Result<(), String> {
        self.values
            .borrow_mut()
            .retain(|v| !environment_variable_manager::same_name(&v.name, name));
        Ok(())
    }
}
fn fixture() -> (Registry, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-recovery-{}-{}-{}",
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

    (
        Registry {
            values: RefCell::new(vec![Variable::text(Scope::User, "Owned", "before", false)]),
            fail_recheck: Cell::new(0),
        },
        path,
    )
}
#[test]
fn explicit_refresh_classifies_pending_without_replaying_and_deduplicates_later_signals() {
    for (observed, outcome) in [
        ("before", "not-applied"),
        ("desired", "applied"),
        ("later external", "uncertain"),
    ] {
        let (registry, path) = fixture();
        let mut app = Application::open(&registry, &path);
        app.edit_entries(Scope::User, "Owned", vec!["desired".into()])
            .unwrap();
        registry.fail_recheck.set(2);
        assert!(app.save(&registry, Scope::User, "Owned").is_err());
        assert_eq!(app.pending_operations().unwrap().len(), 1);
        registry
            .write_user(&Variable::text(Scope::User, "Owned", observed, false))
            .unwrap();
        drop(app);
        let mut app = Application::open(&registry, &path);
        assert!(app.storage_error().is_some());
        app.observe(&registry);
        app.observe_gap(&registry);
        assert_eq!(
            app.pending_operations().unwrap().len(),
            1,
            "A watcher cannot acknowledge recovery"
        );
        app.refresh(&registry);
        assert_eq!(app.storage_error(), None);
        assert!(app.pending_operations().unwrap().is_empty());
        let history = app.history(None, 0, 100).unwrap();
        assert_eq!(
            history.len(),
            1,
            "The recovery snapshot must not become a duplicate external change"
        );
        assert_eq!(history[0].outcome, outcome);
        assert!(history[0].source.contains("recovery gap"));
        assert_eq!(
            history[0].after.as_ref().unwrap().raw_text().as_deref(),
            Some(observed)
        );
        assert_eq!(
            registry.read_scope(Scope::User).unwrap()[0]
                .raw_text()
                .as_deref(),
            Some(observed)
        );
        app.refresh(&registry);
        app.observe(&registry);
        assert_eq!(app.history(None, 0, 100).unwrap(), history);
        drop(app);
        assert_eq!(
            Application::open(&registry, &path)
                .history(None, 0, 100)
                .unwrap(),
            history
        );
        std::fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn read_only_history_keeps_original_data_queryable_and_blocks_all_mutations() {
    for operation in ["create", "edit", "delete"] {
        let (registry, path) = fixture();
        let mut app = Application::open(&registry, &path);
        app.edit_entries(Scope::User, "Owned", vec!["recorded".into()])
            .unwrap();
        app.save(&registry, Scope::User, "Owned").unwrap();
        let history = app.history(None, 0, 100).unwrap();
        drop(app);
        let database = path.join("environment.sqlite");
        let original = std::fs::read(&database).unwrap();
        let mut permissions = std::fs::metadata(&database).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&database, permissions.clone()).unwrap();
        let mut app = Application::open(&registry, &path);
        assert!(
            app.storage_error().is_some(),
            "Read-only startup must report recording interruption before Save"
        );
        assert_eq!(
            app.history(None, 0, 100).unwrap(),
            history,
            "Readable history must use the same original database"
        );
        let before = registry.read_scope(Scope::User).unwrap();
        let result = match operation {
            "create" => {
                app.create_user("New").unwrap();
                app.save(&registry, Scope::User, "New")
            }
            "edit" => {
                app.edit_entries(Scope::User, "Owned", vec!["blocked".into()])
                    .unwrap();
                app.save(&registry, Scope::User, "Owned")
            }
            _ => {
                app.request_delete(Scope::User, "Owned").unwrap();
                app.confirm_delete(&registry)
            }
        };
        assert!(result.is_err());
        assert!(app.storage_error().is_some());
        assert_eq!(registry.read_scope(Scope::User).unwrap(), before);
        assert_eq!(std::fs::read(&database).unwrap(), original);
        app.discard(
            &registry,
            Scope::User,
            if operation == "create" {
                "New"
            } else {
                "Owned"
            },
        )
        .unwrap();
        assert!(app.drafts().next().is_none());
        permissions.set_readonly(false);
        std::fs::set_permissions(&database, permissions).unwrap();
        app.refresh(&registry);
        assert_eq!(app.storage_error(), None);
        assert_eq!(app.history(None, 0, 100).unwrap(), history);
        std::fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn locked_intent_has_bounded_wait_and_recovers_external_gap_without_duplicate_history() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["blocked".into()])
        .unwrap();
    let lock = rusqlite::Connection::open(path.join("environment.sqlite")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    assert!(app.save(&registry, Scope::User, "Owned").is_err());
    assert!(started.elapsed() < std::time::Duration::from_millis(1800));
    assert_eq!(
        registry.read_scope(Scope::User).unwrap()[0]
            .raw_text()
            .as_deref(),
        Some("before")
    );
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    registry
        .write_user(&Variable::text(Scope::User, "Owned", "external", false))
        .unwrap();
    app.observe(&registry);
    assert_eq!(
        app.select(Scope::User, "Owned")
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("external")
    );
    app.discard(&registry, Scope::User, "Owned").unwrap();
    lock.execute_batch("ROLLBACK").unwrap();
    drop(lock);
    app.refresh(&registry);
    assert_eq!(app.storage_error(), None);
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, "gap-net");
    assert_eq!(
        history[0].before.as_ref().unwrap().raw_text().as_deref(),
        Some("before")
    );
    assert_eq!(
        history[0].after.as_ref().unwrap().raw_text().as_deref(),
        Some("external")
    );
    app.refresh(&registry);
    app.observe(&registry);
    assert_eq!(app.history(None, 0, 100).unwrap(), history);
    std::fs::remove_dir_all(path).unwrap();
}
// SQLite's page cap is connection-local. This test-only auto-extension applies
// a real max_page_count to every connection to one disposable database. It does
// not simulate a full physical volume and never changes production policy.
static PAGE_CAP: std::sync::Mutex<Option<(std::ffi::CString, u32)>> = std::sync::Mutex::new(None);
unsafe extern "C" fn cap_database(
    db: *mut rusqlite::ffi::sqlite3,
    _: *mut *mut std::ffi::c_char,
    _: *const rusqlite::ffi::sqlite3_api_routines,
) -> std::ffi::c_int {
    let cap = PAGE_CAP.lock().unwrap();
    if let Some((filename, pages)) = &*cap {
        let actual = rusqlite::ffi::sqlite3_db_filename(db, c"main".as_ptr());
        if !actual.is_null() && std::ffi::CStr::from_ptr(actual) == filename.as_c_str() {
            let pragma = std::ffi::CString::new(format!("PRAGMA max_page_count={pages}")).unwrap();
            return rusqlite::ffi::sqlite3_exec(
                db,
                pragma.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }
    rusqlite::ffi::SQLITE_OK
}
fn enable_page_cap(path: &std::path::Path) {
    let db = rusqlite::Connection::open(path.join("environment.sqlite")).unwrap();
    let pages: u32 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    let filename = unsafe {
        std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_db_filename(
            db.handle(),
            c"main".as_ptr(),
        ))
        .to_owned()
    };
    drop(db);
    *PAGE_CAP.lock().unwrap() = Some((filename, pages));
    unsafe {
        assert_eq!(
            rusqlite::ffi::sqlite3_auto_extension(Some(cap_database)),
            rusqlite::ffi::SQLITE_OK
        );
    }
}
#[test]
fn sqlite_page_cap_full_blocks_before_windows_and_refresh_recovers() {
    for operation in ["create", "edit", "delete"] {
        let (registry, path) = fixture();
        if operation == "delete" {
            registry
                .write_user(&Variable::text(
                    Scope::User,
                    "Owned",
                    &"large".repeat(20_000),
                    false,
                ))
                .unwrap();
        }
        let before = registry.read_scope(Scope::User).unwrap();
        let mut app = Application::open(&registry, &path);
        enable_page_cap(&path);
        let error = match operation {
            "create" => {
                app.create_user("New").unwrap();
                app.edit_entries(Scope::User, "New", vec!["large".repeat(20_000)])
                    .unwrap();
                app.save(&registry, Scope::User, "New")
            }
            "edit" => {
                app.edit_entries(Scope::User, "Owned", vec!["large".repeat(20_000)])
                    .unwrap();
                app.save(&registry, Scope::User, "Owned")
            }
            _ => {
                app.request_delete(Scope::User, "Owned").unwrap();
                app.confirm_delete(&registry)
            }
        }
        .unwrap_err();
        assert!(error.contains("database or disk is full"), "{error}");
        assert_eq!(registry.read_scope(Scope::User).unwrap(), before);
        assert!(app.pending_operations().unwrap().is_empty());
        assert!(app.storage_error().is_some());
        *PAGE_CAP.lock().unwrap() = None;
        registry
            .write_user(&Variable::text(
                Scope::User,
                "Owned",
                "external after full",
                false,
            ))
            .unwrap();
        app.refresh(&registry);
        assert_eq!(app.storage_error(), None);
        assert_eq!(app.history(None, 0, 100).unwrap()[0].outcome, "gap-net");
        std::fs::remove_dir_all(path).unwrap();
    }
    // A separate real FULL occurs after the intent and Windows write succeed.
    struct FullAfterWrite {
        inner: Registry,
        path: PathBuf,
    }
    impl RegistryReader for FullAfterWrite {
        fn read_scope(&self, s: Scope) -> Result<Vec<Variable>, String> {
            self.inner.read_scope(s)
        }
    }
    impl RegistryWriter for FullAfterWrite {
        fn write_user(&self, v: &Variable) -> Result<(), String> {
            self.inner.write_user(v)?;
            enable_page_cap(&self.path);
            Ok(())
        }
    }
    let (inner, path) = fixture();
    let registry = FullAfterWrite {
        inner,
        path: path.clone(),
    };
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["large".repeat(20_000)])
        .unwrap();
    let error = app.save(&registry, Scope::User, "Owned").unwrap_err();
    assert!(
        error.contains("Applied: Windows currently matches")
            && error.contains("database or disk is full"),
        "{error}"
    );
    assert_eq!(app.pending_operations().unwrap().len(), 1);
    assert!(!app.can_save(Scope::User, "Owned"));
    *PAGE_CAP.lock().unwrap() = None;
    registry
        .inner
        .write_user(&Variable::text(
            Scope::User,
            "Owned",
            "later external",
            false,
        ))
        .unwrap();
    app.refresh(&registry);
    assert_eq!(app.storage_error(), None);
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, "uncertain");
    assert_eq!(
        history[0].after.as_ref().unwrap().raw_text().as_deref(),
        Some("later external")
    );
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn existing_corrupt_and_share_denied_database_are_preserved_without_replacement() {
    use std::os::windows::fs::OpenOptionsExt;
    for corrupt in [false, true] {
        let (registry, path) = fixture();
        drop(Application::open(&registry, &path));
        let database = path.join("environment.sqlite");
        if corrupt {
            std::fs::write(&database, b"owned corrupt fixture, preserve exactly").unwrap();
        }
        let original = std::fs::read(&database).unwrap();
        let mut original_contents: Vec<_> = std::fs::read_dir(&path)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        original_contents.sort();
        let held = if corrupt {
            None
        } else {
            Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&database)
                    .unwrap(),
            )
        };
        let mut app = Application::open(&registry, &path);
        assert!(app.storage_error().is_some());
        assert_eq!(app.browse("Owned", None).len(), 1);
        app.edit_entries(Scope::User, "Owned", vec!["blocked".into()])
            .unwrap();
        assert!(app.save(&registry, Scope::User, "Owned").is_err());
        app.refresh(&registry);
        assert!(app.storage_error().is_some());
        app.discard(&registry, Scope::User, "Owned").unwrap();
        drop(held);
        assert_eq!(std::fs::read(&database).unwrap(), original);
        if !corrupt {
            app.refresh(&registry);
            assert_eq!(app.storage_error(), None);
        }
        let mut contents: Vec<_> = std::fs::read_dir(&path)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        contents.sort();
        assert_eq!(
            contents, original_contents,
            "No replacement database or alternate folder"
        );
        std::fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn live_backup_reopens_with_identical_history_and_never_overwrites_existing_files() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["saved before backup".into()])
        .unwrap();
    app.save(&registry, Scope::User, "Owned").unwrap();
    let expected = app.history(None, 0, 100).unwrap();
    let backup_directory = path.join("backup");
    std::fs::create_dir(&backup_directory).unwrap();
    let backup = backup_directory.join("environment.sqlite");
    app.backup_history(&backup).unwrap();
    let restored = Application::open(&registry, &backup_directory);
    assert_eq!(restored.storage_error(), None);
    assert_eq!(restored.history(None, 0, 100).unwrap(), expected);
    assert!(restored.pending_operations().unwrap().is_empty());
    let original = std::fs::read(&backup).unwrap();
    assert!(app.backup_history(&backup).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn write_rejection_reports_not_applied_and_preserves_the_original_snapshot() {
    struct Reject(Registry);
    impl RegistryReader for Reject {
        fn read_scope(&self, s: Scope) -> Result<Vec<Variable>, String> {
            self.0.read_scope(s)
        }
    }
    impl RegistryWriter for Reject {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            Err("Windows denied write".into())
        }
    }
    let (registry, path) = fixture();
    let registry = Reject(registry);
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["desired".into()])
        .unwrap();
    assert!(app
        .save(&registry, Scope::User, "Owned")
        .unwrap_err()
        .contains("Not applied"));
    assert_eq!(app.history(None, 0, 100).unwrap()[0].outcome, "not-applied");
    assert!(app.storage_error().is_some());
    app.refresh(&registry);
    assert_eq!(app.storage_error(), None);
    assert_eq!(app.history(None, 0, 100).unwrap().len(), 1);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn refresh_never_replaces_a_database_that_disappeared_during_the_session() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["recorded".into()])
        .unwrap();
    app.save(&registry, Scope::User, "Owned").unwrap();
    let history = app.history(None, 0, 100).unwrap();
    let database = path.join("environment.sqlite");
    let held = path.join("original.sqlite");
    std::fs::rename(&database, &held).unwrap();
    app.edit_entries(Scope::User, "Owned", vec!["blocked".into()])
        .unwrap();
    assert!(app.save(&registry, Scope::User, "Owned").is_err());
    app.refresh(&registry);
    assert!(app.storage_error().is_some());
    assert!(
        !database.exists(),
        "Refresh cannot silently create replacement history"
    );
    let reopened = Application::open(&registry, &path);
    assert!(reopened.storage_error().is_some());
    assert!(!database.exists());
    std::fs::rename(&held, &database).unwrap();
    app.refresh(&registry);
    assert_eq!(app.storage_error(), None);
    assert_eq!(app.history(None, 0, 100).unwrap(), history);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn unreadable_scope_never_classifies_a_pending_delete_as_applied_absence() {
    struct Denied;
    impl RegistryReader for Denied {
        fn read_scope(&self, s: Scope) -> Result<Vec<Variable>, String> {
            if s == Scope::User {
                Err("denied".into())
            } else {
                Ok(Vec::new())
            }
        }
    }
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.request_delete(Scope::User, "Owned").unwrap();
    registry.fail_recheck.set(2);
    assert!(app.confirm_delete(&registry).is_err());
    let pending = app.pending_operations().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].desired_after, None);
    app.refresh(&Denied);
    assert!(app.storage_error().is_some());
    assert_eq!(app.pending_operations().unwrap(), pending);
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    assert!(app.select(Scope::User, "Owned").is_some());
    app.refresh(&registry);
    assert_eq!(app.storage_error(), None);
    assert_eq!(app.history(None, 0, 100).unwrap()[0].outcome, "not-applied");
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn partial_write_results_report_observed_applied_or_uncertain_without_duplicate_gap() {
    struct Partial {
        inner: Registry,
        other: bool,
    }
    impl RegistryReader for Partial {
        fn read_scope(&self, s: Scope) -> Result<Vec<Variable>, String> {
            self.inner.read_scope(s)
        }
    }
    impl RegistryWriter for Partial {
        fn write_user(&self, v: &Variable) -> Result<(), String> {
            if self.other {
                self.inner.write_user(&Variable::text(
                    Scope::User,
                    "Owned",
                    "raced external",
                    false,
                ))?;
            } else {
                self.inner.write_user(v)?;
            }
            Err("write completion failed".into())
        }
    }
    for other in [false, true] {
        let (inner, path) = fixture();
        let registry = Partial { inner, other };
        let mut app = Application::open(&registry, &path);
        app.edit_entries(Scope::User, "Owned", vec!["desired".into()])
            .unwrap();
        let error = app.save(&registry, Scope::User, "Owned").unwrap_err();
        assert!(
            error.starts_with(if other { "Uncertain:" } else { "Applied:" }),
            "{error}"
        );
        let history = app.history(None, 0, 100).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].outcome,
            if other { "uncertain" } else { "applied" }
        );
        assert!(app.storage_error().is_some());
        app.refresh(&registry);
        assert_eq!(app.storage_error(), None);
        assert_eq!(app.history(None, 0, 100).unwrap(), history);
        std::fs::remove_dir_all(path).unwrap();
    }
}
