use environment_variable_manager::{Application, RegistryReader, RegistryWriter, Scope, Variable};
use std::{cell::RefCell, path::PathBuf};
struct Registry(RefCell<Vec<Variable>>);
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(self
            .0
            .borrow()
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
impl RegistryWriter for Registry {
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        *self.0.borrow_mut() = vec![value.clone()];
        Ok(())
    }
}
fn fixture() -> (Registry, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-resume-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    (
        Registry(RefCell::new(vec![Variable::text(
            Scope::User,
            "Owned",
            "before",
            false,
        )])),
        path,
    )
}
#[test]
fn interrupted_notifications_block_all_writes_and_preserve_drafts_until_healthy() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["draft".into()])
        .unwrap();
    app.set_monitoring_errors(vec![(Scope::System, "Resume rearm failed".into())]);
    assert!(!app.can_save(Scope::User, "Owned"));
    assert!(app.save(&registry, Scope::User, "Owned").is_err());
    app.request_delete(Scope::User, "Owned").unwrap();
    assert!(app.confirm_delete(&registry).is_err());
    assert_eq!(
        registry.read_scope(Scope::User).unwrap()[0]
            .raw_text()
            .as_deref(),
        Some("before")
    );
    assert!(app.pending_operations().unwrap().is_empty());
    assert_eq!(
        app.draft(Scope::User, "Owned").unwrap().entries,
        vec!["draft"]
    );
    app.set_monitoring_errors(vec![]);
    app.observe_gap(&registry);
    assert!(app.can_save(Scope::User, "Owned"));
    app.save(&registry, Scope::User, "Owned").unwrap();
    assert_eq!(app.history(None, 0, 10).unwrap().len(), 1);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn successive_power_events_rearm_real_notifications_and_label_only_net_changes() {
    use environment_variable_manager::{resume::Monitoring, windows_registry::WindowsRegistry};
    use std::{
        ptr::{null, null_mut},
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS, System::Registry::*, UI::WindowsAndMessaging::*,
    };
    let (unused, path) = fixture();
    drop(unused);
    let root = format!(
        "Software\\EnvironmentManagerTests\\{}",
        path.file_name().unwrap().to_str().unwrap()
    );
    let paths = [format!("{root}\\User"), format!("{root}\\System")];
    let mut owned_keys = Vec::new();
    unsafe {
        for name in [&root, &paths[0], &paths[1]] {
            let mut key = null_mut();
            let mut disposition = 0;
            assert_eq!(
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    name.encode_utf16()
                        .chain(Some(0))
                        .collect::<Vec<_>>()
                        .as_ptr(),
                    0,
                    null(),
                    0,
                    KEY_ALL_ACCESS,
                    null(),
                    &mut key,
                    &mut disposition
                ),
                ERROR_SUCCESS
            );
            assert_eq!(disposition, REG_CREATED_NEW_KEY, "Exclusive fixture only");
            owned_keys.push(key);
        }
    }
    let registry = WindowsRegistry::under_current_user(&paths[0], &paths[1]);
    registry
        .write_user(&Variable::text(Scope::User, "Owned", "before", false))
        .unwrap();
    let mut watch = registry.watch();
    let mut monitor = Monitoring::default();
    monitor.poll(Some(&mut watch));
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["unsaved".into()])
        .unwrap();
    // Deny only notification access on our exclusively owned System key. Reads
    // remain possible; the last persisted baseline must not move until rearm succeeds.
    unsafe {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
                DACL_SECURITY_INFORMATION,
            },
        };
        let mut length = 0;
        RegGetKeySecurity(
            owned_keys[2],
            DACL_SECURITY_INFORMATION,
            null_mut(),
            &mut length,
        );
        assert!(length > 0);
        let mut original = vec![0u8; length as usize];
        assert_eq!(
            RegGetKeySecurity(
                owned_keys[2],
                DACL_SECURITY_INFORMATION,
                original.as_mut_ptr().cast(),
                &mut length
            ),
            ERROR_SUCCESS
        );
        let mut descriptor = null_mut();
        let sddl: Vec<u16> = "D:P(D;;0x10;;;WD)(A;;KA;;;WD)"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut()
            ),
            0
        );
        assert_eq!(
            RegSetKeySecurity(owned_keys[2], DACL_SECURITY_INFORMATION, descriptor),
            ERROR_SUCCESS
        );
        LocalFree(descriptor);
        monitor.power_event(PBT_APMRESUMEAUTOMATIC);
        monitor.poll(Some(&mut watch));
        let denied = monitor.errors.clone();
        assert_eq!(
            RegSetKeySecurity(
                owned_keys[2],
                DACL_SECURITY_INFORMATION,
                original.as_mut_ptr().cast()
            ),
            ERROR_SUCCESS
        );
        assert!(denied.iter().any(|(scope, _)| *scope == Scope::System));
        app.set_monitoring_errors(denied);
        assert!(!app.can_save(Scope::User, "Owned"));
        app.observe_gap(&registry);
        assert!(app.history(None, 0, 10).unwrap().is_empty());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !monitor.errors.is_empty() && std::time::Instant::now() < deadline {
            monitor.poll(Some(&mut watch));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            monitor.errors.is_empty(),
            "Ordinary polling recovers without another power event"
        );
        assert!(monitor.gap && monitor.changed);
        app.set_monitoring_errors(vec![]);
        app.observe_gap(&registry);
    }
    for raw in ["before", "after sleep", "after hibernate"] {
        assert!(monitor.power_event(PBT_APMSUSPEND));
        monitor.poll(Some(&mut watch));
        assert!(!monitor.errors.is_empty());
        app.set_monitoring_errors(monitor.errors.clone());
        assert!(!app.can_save(Scope::User, "Owned"));
        registry
            .write_user(&Variable::text(Scope::User, "Owned", raw, false))
            .unwrap();
        assert!(monitor.power_event(PBT_APMRESUMEAUTOMATIC));
        monitor.poll(Some(&mut watch));
        assert!(monitor.gap && monitor.changed && monitor.errors.is_empty());
        app.set_monitoring_errors(monitor.errors.clone());
        app.observe_gap(&registry);
        assert!(monitor.power_event(PBT_APMRESUMESUSPEND));
        monitor.poll(Some(&mut watch));
        app.observe_gap(&registry);
        app.refresh(&registry);
        app.observe(&registry);
    }
    let history = app.history(None, 0, 10).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|r| r.outcome == "gap-net"
        && r.time_ms > 0
        && r.source == "external (actor unknown)"));
    assert_eq!(
        history[1].before.as_ref().unwrap().raw_text().as_deref(),
        Some("before")
    );
    assert_eq!(
        history[0].after.as_ref().unwrap().raw_text().as_deref(),
        Some("after hibernate")
    );
    assert_eq!(
        app.draft(Scope::User, "Owned").unwrap().entries,
        vec!["unsaved"]
    );
    assert!(app.draft(Scope::User, "Owned").unwrap().conflict);
    assert!(app.save(&registry, Scope::User, "Owned").is_err());
    registry
        .write_user(&Variable::text(
            Scope::User,
            "Owned",
            "live after recovery",
            false,
        ))
        .unwrap();
    assert!(
        watch.poll(Duration::from_secs(2)).changed,
        "Real notification remains active after repeated rearm"
    );
    app.observe(&registry);
    assert_eq!(app.history(None, 0, 10).unwrap()[0].outcome, "observed");
    assert!(!monitor.power_event(PBT_APMPOWERSTATUSCHANGE));
    assert_eq!(
        registry.read_scope(Scope::User).unwrap(),
        vec![Variable::text(
            Scope::User,
            "Owned",
            "live after recovery",
            false
        )]
    );
    assert!(registry.read_scope(Scope::System).unwrap().is_empty());
    drop(watch);
    for key in owned_keys {
        unsafe {
            RegCloseKey(key);
        }
    }
    unsafe {
        assert_eq!(
            RegDeleteTreeW(
                HKEY_CURRENT_USER,
                root.encode_utf16()
                    .chain(Some(0))
                    .collect::<Vec<_>>()
                    .as_ptr()
            ),
            ERROR_SUCCESS
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn failed_resume_keeps_baseline_until_observation_is_healthy() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    let before = app.baseline().unwrap().to_vec();
    app.set_monitoring_errors(vec![(Scope::User, "Cannot rearm notifications".into())]);
    registry
        .write_user(&Variable::text(Scope::User, "Owned", "later", false))
        .unwrap();
    app.observe_gap(&registry);
    assert_eq!(app.baseline().unwrap(), before);
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    app.set_monitoring_errors(vec![]);
    app.observe_gap(&registry);
    let history = app.history(None, 0, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, "gap-net");
    assert_eq!(
        history[0].before.as_ref().unwrap().raw_text().as_deref(),
        Some("before")
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn resume_storage_failure_retains_baseline_and_draft_until_explicit_recovery() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::User, "Owned", vec!["draft".into()])
        .unwrap();
    let before = app.baseline().unwrap().to_vec();
    let lock = rusqlite::Connection::open(path.join("environment.sqlite")).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    registry
        .write_user(&Variable::text(
            Scope::User,
            "Owned",
            "across power gap",
            false,
        ))
        .unwrap();
    app.observe_gap(&registry);
    assert!(app.storage_error().is_some());
    assert_eq!(app.baseline().unwrap(), before);
    assert!(!app.can_save(Scope::User, "Owned"));
    lock.execute_batch("ROLLBACK").unwrap();
    drop(lock);
    registry
        .write_user(&Variable::text(Scope::User, "Owned", "latest", false))
        .unwrap();
    app.observe_gap(&registry);
    app.observe(&registry);
    assert!(
        app.storage_error().is_some(),
        "Automatic resume never lifts a recording gate"
    );
    assert_eq!(app.baseline().unwrap(), before);
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    app.refresh(&registry);
    assert!(app.storage_error().is_none());
    let history = app.history(None, 0, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, "gap-net");
    assert_eq!(
        history[0].before.as_ref().unwrap().raw_text().as_deref(),
        Some("before")
    );
    assert_eq!(
        history[0].after.as_ref().unwrap().raw_text().as_deref(),
        Some("latest")
    );
    let draft = app.draft(Scope::User, "Owned").unwrap();
    assert_eq!(draft.entries, vec!["draft"]);
    assert!(draft.conflict);
    assert!(app.save(&registry, Scope::User, "Owned").is_err());
    std::fs::remove_dir_all(path).unwrap();
}
