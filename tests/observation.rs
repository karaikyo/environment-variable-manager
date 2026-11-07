use environment_variable_manager::{Application, RegistryReader, Scope, Variable};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
};
struct Registry {
    values: RefCell<Vec<Variable>>,
    denied: Cell<bool>,
}
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        if scope == Scope::User && self.denied.get() {
            return Err("Access denied".into());
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
fn fixture() -> (Registry, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "evm-observe-{}-{}-{}",
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
        directory.display()
    );
    std::fs::create_dir(&directory)
        .expect("Exclusively reserve owned test directory; abort on collision");
    (
        Registry {
            values: RefCell::new(vec![Variable::text(Scope::User, "Owned", "before", false)]),
            denied: Cell::new(false),
        },
        directory,
    )
}
#[test]
fn observed_changes_preserve_raw_state_and_repeated_signals_are_deduplicated() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    let before = registry.values.borrow()[0].clone();
    let after = Variable::text(Scope::User, "Owned", "", true);
    *registry.values.borrow_mut() = vec![
        after.clone(),
        Variable::text(Scope::System, "Same", "%RAW%; ", true),
    ];
    app.observe(&registry);
    app.observe(&registry);
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 2);
    let change = history.iter().find(|r| r.name == "Owned").unwrap();
    assert_eq!(change.before, Some(before));
    assert_eq!(change.after, Some(after));
    assert_eq!(change.source, "external (actor unknown)");
    assert_eq!(change.outcome, "observed");
    assert!(change.time_ms > 0);
    assert_eq!(
        app.select(Scope::User, "Owned")
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restart_and_refresh_record_only_net_gap_differences() {
    let (registry, directory) = fixture();
    drop(Application::open(&registry, &directory));
    *registry.values.borrow_mut() = vec![Variable::text(Scope::User, "Owned", "later", false)];
    let mut app = Application::open(&registry, &directory);
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, "gap-net");
    assert_eq!(
        history[0].before.as_ref().unwrap().raw_text().as_deref(),
        Some("before")
    );
    assert_eq!(
        history[0].after.as_ref().unwrap().raw_text().as_deref(),
        Some("later")
    );
    app.refresh(&registry);
    registry.values.borrow_mut().clear();
    app.refresh(&registry);
    app.observe(&registry);
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].after, None);
    assert_eq!(history[0].outcome, "gap-net");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn read_failures_keep_last_values_and_drafts_until_gap_reconciliation() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "Owned", vec!["unsaved".into()])
        .unwrap();
    registry.denied.set(true);
    registry.values.borrow_mut().clear();
    app.observe(&registry);
    assert_eq!(app.scope_error(Scope::User), Some("Access denied"));
    assert!(app.select(Scope::User, "Owned").is_some());
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    registry.denied.set(false);
    app.observe(&registry);
    assert!(app.scope_error(Scope::User).is_none());
    assert!(app.select(Scope::User, "Owned").is_none());
    let draft = app.draft(Scope::User, "Owned").unwrap();
    assert_eq!(draft.entries, vec!["unsaved"]);
    assert!(draft.conflict && !app.can_save(Scope::User, "Owned"));
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, "gap-net");
    assert!(history[0].after.is_none());
    app.discard(&registry, Scope::User, "Owned").unwrap();
    assert!(app.draft(Scope::User, "Owned").is_none());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn recording_interruption_reconciles_net_state_after_explicit_refresh() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    let lock = rusqlite::Connection::open(directory.join("environment.sqlite")).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    *registry.values.borrow_mut() =
        vec![Variable::text(Scope::User, "Owned", "intermediate", false)];
    app.observe(&registry);
    assert!(app.storage_error().is_some());
    assert_eq!(
        app.select(Scope::User, "Owned")
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("intermediate")
    );
    lock.execute_batch("ROLLBACK").unwrap();
    drop(lock);
    *registry.values.borrow_mut() = vec![Variable::text(Scope::User, "Owned", "latest", false)];
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
        Some("latest")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rapid_net_changes_and_creation_draft_collisions_do_not_fabricate_intermediate_edits() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    let original = registry.values.borrow().clone();
    *registry.values.borrow_mut() = vec![Variable::text(
        Scope::User,
        "Owned",
        "unobserved intermediate",
        false,
    )];
    *registry.values.borrow_mut() = original;
    app.observe(&registry);
    assert!(app.history(None, 0, 100).unwrap().is_empty());
    app.create_user("New").unwrap();
    app.edit_entries(Scope::User, "New", vec!["draft text".into()])
        .unwrap();
    registry
        .values
        .borrow_mut()
        .push(Variable::text(Scope::User, "NEW", "external", false));
    app.observe(&registry);
    let draft = app.draft(Scope::User, "New").unwrap();
    assert!(draft.conflict);
    assert_eq!(draft.entries, vec!["draft text"]);
    assert!(!app.can_save(Scope::User, "New"));
    let history = app.history(None, 0, 100).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].before, None);
    assert_eq!(history[0].source, "external (actor unknown)");
    std::fs::remove_dir_all(directory).unwrap();
}
