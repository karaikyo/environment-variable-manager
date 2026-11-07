use environment_variable_manager::{
    lifecycle::{ExitDecision, Lifecycle},
    Application, RegistryReader, Scope, Variable,
};
struct Registry;
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(if scope == Scope::User {
            vec![
                Variable::text(scope, "A", "one", false),
                Variable::text(scope, "B", "two", false),
            ]
        } else {
            vec![]
        })
    }
}
fn directory() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-lifecycle-{}-{}-{}",
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
fn tray_reopen_cancel_and_confirm_keep_saved_values_and_all_session_drafts() {
    let dir = directory();
    let mut app = Application::open(&Registry, &dir);
    app.edit_entries(Scope::User, "A", vec!["changed".into()])
        .unwrap();
    app.create_user("New").unwrap();
    let mut lifecycle = Lifecycle::default();
    assert!(lifecycle.close_to_tray(true));
    assert!(!lifecycle.visible());
    lifecycle.reopen();
    assert!(lifecycle.visible());
    assert_eq!(
        lifecycle.request_exit(Some(&app)),
        ExitDecision::ConfirmDiscard
    );
    lifecycle.cancel_exit();
    assert!(app.draft(Scope::User, "A").is_some());
    assert!(app.draft(Scope::User, "New").is_some());
    assert_eq!(
        lifecycle.request_exit(Some(&app)),
        ExitDecision::ConfirmDiscard
    );
    lifecycle.confirm_exit(&mut app);
    assert!(!app.has_unsaved_edits());
    assert_eq!(
        app.select(Scope::User, "A").unwrap().raw_text().as_deref(),
        Some("one")
    );
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn exit_waits_for_the_worker_ignores_reverted_edits_and_rejects_unreachable_tray() {
    let dir = directory();
    let mut app = Application::open(&Registry, &dir);
    let mut life = Lifecycle::default();
    assert!(!life.close_to_tray(false));
    assert!(life.visible());
    assert_eq!(life.request_exit(None), ExitDecision::WaitForOperation);
    app.edit_entries(Scope::User, "B", vec!["temporary".into()])
        .unwrap();
    app.edit_entries(Scope::User, "B", vec!["two".into()])
        .unwrap();
    assert_eq!(life.request_exit(Some(&app)), ExitDecision::Exit);
    app.edit_entries(Scope::User, "B", vec![]).unwrap();
    assert_eq!(life.request_exit(Some(&app)), ExitDecision::ConfirmDiscard);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn single_instance_claim_prevents_a_second_observer_until_owner_exits() {
    use environment_variable_manager::native_lifecycle::Instance;
    let name = format!(
        "Local\\EvmOwnedInstanceTest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let first = Instance::claim(&name).unwrap().unwrap();
    assert!(Instance::claim(&name).unwrap().is_none());
    drop(first);
    assert!(Instance::claim(&name).unwrap().is_some());
}
#[test]
fn confirmed_exit_never_erases_a_saved_change_or_unresolved_persisted_intent() {
    use environment_variable_manager::RegistryWriter;
    use std::cell::{Cell, RefCell};
    struct Boundary {
        values: RefCell<Vec<Variable>>,
        deny: Cell<bool>,
    }
    impl RegistryReader for Boundary {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            if self.deny.get() {
                return Err("Owned readback denial".into());
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
    impl RegistryWriter for Boundary {
        fn write_user(&self, v: &Variable) -> Result<(), String> {
            let mut values = self.values.borrow_mut();
            values.retain(|old| old.name != v.name);
            values.push(v.clone());
            if v.name == "Pending" {
                self.deny.set(true);
            }
            Ok(())
        }
    }
    let dir = directory();
    let registry = Boundary {
        values: RefCell::new(vec![
            Variable::text(Scope::User, "Saved", "before", false),
            Variable::text(Scope::User, "Pending", "before", false),
        ]),
        deny: Cell::new(false),
    };
    let mut app = Application::open(&registry, &dir);
    app.edit_entries(Scope::User, "Saved", vec!["applied".into()])
        .unwrap();
    app.save(&registry, Scope::User, "Saved").unwrap();
    app.edit_entries(Scope::User, "Pending", vec!["possibly applied".into()])
        .unwrap();
    assert!(app.save(&registry, Scope::User, "Pending").is_err());
    let pending = app.pending_operations().unwrap();
    assert_eq!(pending.len(), 1);
    let history = app.history(None, 0, 10).unwrap();
    assert_eq!(history.len(), 1);
    app.create_user("UnselectedNew").unwrap();
    let mut life = Lifecycle::default();
    assert_eq!(life.request_exit(Some(&app)), ExitDecision::ConfirmDiscard);
    life.confirm_exit(&mut app);
    assert_eq!(app.pending_operations().unwrap(), pending);
    assert_eq!(app.history(None, 0, 10).unwrap(), history);
    drop(app);
    registry.deny.set(false);
    let mut reopened = Application::open(&registry, &dir);
    assert!(reopened.drafts().next().is_none());
    assert_eq!(reopened.pending_operations().unwrap(), pending);
    assert_eq!(
        reopened
            .select(Scope::User, "Saved")
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("applied")
    );
    reopened.refresh(&registry);
    assert!(reopened.pending_operations().unwrap().is_empty());
    assert_eq!(reopened.history(None, 0, 10).unwrap().len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn closing_to_tray_cancels_an_exit_waiting_for_the_current_operation() {
    let mut life = Lifecycle::default();
    assert_eq!(life.request_exit(None), ExitDecision::WaitForOperation);
    assert!(life.close_to_tray(true));
    assert!(!life.visible());
    assert!(!life.exit_requested());
    assert!(!life.close_to_tray(false));
    assert!(life.visible());
}
