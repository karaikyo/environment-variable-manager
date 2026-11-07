use environment_variable_manager::{
    Application, RegistryReader, RegistryWriter, SaveOutcome, Scope, Variable,
};
use std::cell::RefCell;
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
    fn delete_user(&self, name: &str) -> Result<(), String> {
        self.0.borrow_mut().retain(|v| {
            !(v.scope == Scope::User && environment_variable_manager::same_name(&v.name, name))
        });
        Ok(())
    }
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        self.0.borrow_mut().push(value.clone());
        Ok(())
    }
}
fn directory() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-create-delete-{}-{}-{}",
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
#[test]
fn new_empty_variable_is_present_plain_text_with_creation_history_after_restart() {
    let registry = Registry(RefCell::new(vec![Variable::text(
        Scope::System,
        "New",
        "system",
        true,
    )]));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.create_user("New").unwrap();
    assert!(app.can_save(Scope::User, "new"));
    assert_eq!(
        app.save(&registry, Scope::User, "new"),
        Ok(SaveOutcome::Saved)
    );
    let expected = Variable::text(Scope::User, "New", "", false);
    assert_eq!(app.select(Scope::User, "NEW"), Some(&expected));
    drop(app);
    let reopened = Application::open(&registry, &directory);
    assert!(reopened.baseline().unwrap().contains(&expected));
    let history = reopened.history(Some((Scope::User, "new")), 0, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].before, None);
    assert_eq!(history[0].after, Some(expected));
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn creation_validates_identity_preserves_entries_type_and_selected_discard() {
    let registry = Registry(RefCell::new(vec![Variable::text(
        Scope::User,
        "Existing",
        "original",
        false,
    )]));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "existing", vec!["other draft".into()])
        .unwrap();
    for invalid in ["", "A=B", "nul\0name"] {
        assert!(app.create_user(invalid).is_err());
    }
    assert!(app.create_user("EXISTING").is_err());
    app.create_user("New").unwrap();
    assert!(app.create_user("NEW").is_err());
    app.edit_entries(
        Scope::User,
        "new",
        vec!["%TEMP%".into(), "".into(), " x\nline ".into(), "".into()],
    )
    .unwrap();
    app.set_new_expand("new", true).unwrap();
    app.save(&registry, Scope::User, "NEW").unwrap();
    assert_eq!(
        app.select(Scope::User, "new"),
        Some(&Variable::text(
            Scope::User,
            "New",
            "%TEMP%;; x\nline ;",
            true
        ))
    );
    app.create_user("DiscardMe").unwrap();
    app.discard(&registry, Scope::User, "discardme").unwrap();
    assert!(app.draft(Scope::User, "discardme").is_none());
    assert!(app.select(Scope::User, "discardme").is_none());
    assert_eq!(
        app.draft(Scope::User, "existing").unwrap().entries,
        vec!["other draft"]
    );
    assert_eq!(app.history(None, 0, 10).unwrap().len(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn inline_delete_cancel_is_inert_and_confirmation_records_presence_to_absence() {
    let registry = Registry(RefCell::new(vec![
        Variable::text(Scope::User, "DeleteMe", "", true),
        Variable::text(Scope::User, "Other", "keep", false),
    ]));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "DeleteMe", vec!["unsaved".into()])
        .unwrap();
    app.edit_entries(Scope::User, "Other", vec!["other draft".into()])
        .unwrap();
    app.request_delete(Scope::User, "deleteme").unwrap();
    app.cancel_delete();
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    assert_eq!(
        app.select(Scope::User, "DeleteMe")
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("")
    );
    assert_eq!(
        app.draft(Scope::User, "DeleteMe").unwrap().entries,
        vec!["unsaved"]
    );
    assert!(app.confirm_delete(&registry).is_err());
    app.request_delete(Scope::User, "deleteme").unwrap();
    app.confirm_delete(&registry).unwrap();
    assert!(app.select(Scope::User, "DeleteMe").is_none());
    assert!(app.draft(Scope::User, "DeleteMe").is_none());
    assert_eq!(
        app.draft(Scope::User, "Other").unwrap().entries,
        vec!["other draft"]
    );
    drop(app);
    let reopened = Application::open(&registry, &directory);
    assert!(!reopened
        .baseline()
        .unwrap()
        .iter()
        .any(|v| v.name == "DeleteMe"));
    let history = reopened.history(None, 0, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].before,
        Some(Variable::text(Scope::User, "DeleteMe", "", true))
    );
    assert_eq!(history[0].after, None);
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn unsaved_creation_discard_needs_no_windows_read() {
    struct Denied;
    impl RegistryReader for Denied {
        fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
            Err("read denied".into())
        }
    }
    let registry = Registry(RefCell::new(Vec::new()));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.create_user("New").unwrap();
    app.discard(&Denied, Scope::User, "new").unwrap();
    assert!(app.draft(Scope::User, "new").is_none());
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn deletion_cannot_bypass_a_detected_draft_conflict_even_if_windows_reverts() {
    let original = Variable::text(Scope::User, "Fixture", "original", false);
    let registry = Registry(RefCell::new(vec![original.clone()]));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "fixture", vec!["draft".into()])
        .unwrap();
    registry.0.borrow_mut()[0] = Variable::text(Scope::User, "Fixture", "external", false);
    assert!(app.save(&registry, Scope::User, "fixture").is_err());
    registry.0.borrow_mut()[0] = original;
    assert!(
        app.request_delete(Scope::User, "fixture").is_err(),
        "Discard must resolve an already detected conflict before any mutation"
    );
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn creation_collision_and_stale_delete_are_rechecked_without_overwriting_windows() {
    let registry = Registry(RefCell::new(Vec::new()));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.create_user("New").unwrap();
    let external = Variable::text(Scope::User, "NEW", "external", true);
    registry.0.borrow_mut().push(external.clone());
    assert!(app
        .save(&registry, Scope::User, "new")
        .unwrap_err()
        .contains("Conflict"));
    assert_eq!(
        registry.read_scope(Scope::User).unwrap(),
        vec![external.clone()]
    );
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    app.discard(&registry, Scope::User, "new").unwrap();
    let mut app = Application::open(&registry, &directory);
    app.request_delete(Scope::User, "new").unwrap();
    let later = Variable::text(Scope::User, "NEW", "later external", true);
    registry.0.borrow_mut()[0] = later.clone();
    assert!(app
        .confirm_delete(&registry)
        .unwrap_err()
        .contains("Conflict"));
    assert_eq!(registry.read_scope(Scope::User).unwrap(), vec![later]);
    let history = app.history(None, 0, 10).unwrap();
    assert_eq!(
        history.len(),
        1,
        "Reopening records the external creation gap only"
    );
    assert_eq!(history[0].source, "external (actor unknown)");
    assert_eq!(history[0].outcome, "gap-net");
    assert_eq!(history[0].before, None);
    assert_eq!(history[0].after, Some(external));
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn deletion_requires_durable_intent_before_windows_changes() {
    let original = Variable::text(Scope::User, "Fixture", "keep", false);
    let registry = Registry(RefCell::new(vec![original.clone()]));
    let directory = directory();
    let mut app = Application::open(&registry, &directory);
    app.request_delete(Scope::User, "fixture").unwrap();
    let database = directory.join("environment.sqlite");
    let preserved = directory.join("preserved.sqlite");
    std::fs::rename(&database, &preserved).unwrap();
    std::fs::create_dir(&database).unwrap();
    assert!(app.confirm_delete(&registry).is_err());
    assert_eq!(registry.read_scope(Scope::User).unwrap(), vec![original]);
    assert!(app.storage_error().is_some());
    assert!(app.delete_confirmation().is_some());
    app.cancel_delete();
    std::fs::remove_dir(&database).unwrap();
    std::fs::rename(&preserved, &database).unwrap();
    assert!(app.history(None, 0, 10).unwrap().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
