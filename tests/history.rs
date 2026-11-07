use environment_variable_manager::{
    history_comparison::EntryChange, HistoryRecord, Scope, Variable,
};
fn record(before: Option<&str>, after: Option<&str>) -> HistoryRecord {
    HistoryRecord {
        id: 1,
        scope: Scope::User,
        name: "Fixture".into(),
        before: before.map(|s| Variable::text(Scope::User, "Fixture", s, false)),
        after: after.map(|s| Variable::text(Scope::User, "Fixture", s, false)),
        time_ms: 1,
        source: "observed external".into(),
        outcome: "observed".into(),
    }
}
#[test]
fn comparison_preserves_entries_and_aligns_a_simple_insertion() {
    let view = record(Some(" a;;dup;dup;tail;"), Some(" a;;insert;dup;dup;tail;")).comparison();
    assert!(view.before_present && view.after_present);
    assert_eq!(
        view.rows.iter().map(|r| r.change).collect::<Vec<_>>(),
        vec![
            EntryChange::Unchanged,
            EntryChange::Unchanged,
            EntryChange::Added,
            EntryChange::Unchanged,
            EntryChange::Unchanged,
            EntryChange::Unchanged,
            EntryChange::Unchanged
        ]
    );
    assert_eq!(view.rows[2].before, None);
    assert_eq!(view.rows[2].after.as_ref().unwrap().text, "insert");
    assert_eq!(view.rows[3].before.as_ref().unwrap().number, 3);
    assert_eq!(view.rows[3].after.as_ref().unwrap().number, 4);
    assert_eq!(view.rows.last().unwrap().after.as_ref().unwrap().text, "");
}
#[test]
fn comparison_distinguishes_absence_empty_and_snapshot_edits() {
    let edited = record(Some("old;keep;removed"), Some("new;keep")).comparison();
    assert_eq!(
        edited.rows.iter().map(|r| r.change).collect::<Vec<_>>(),
        vec![
            EntryChange::Edited,
            EntryChange::Unchanged,
            EntryChange::Deleted
        ]
    );
    assert_eq!(edited.rows[0].before.as_ref().unwrap().text, "old");
    let created = record(None, Some("")).comparison();
    assert!(!created.before_present && created.after_present);
    assert_eq!(created.rows.len(), 1);
    assert_eq!(created.rows[0].change, EntryChange::Added);
    assert_eq!(created.rows[0].after.as_ref().unwrap().text, "");
    let removed = record(Some(""), None).comparison();
    assert!(removed.before_present && !removed.after_present);
    assert_eq!(removed.rows[0].change, EntryChange::Deleted);
    let exact = record(Some("x\n\ry;  ;%ROOT%"), Some("x\n\ry;  ;%ROOT%")).comparison();
    assert_eq!(exact.rows[0].after.as_ref().unwrap().text, "x\n\ry");
    assert!(exact
        .rows
        .iter()
        .all(|r| r.change == EntryChange::Unchanged));
}
use environment_variable_manager::{same_name, Application, RegistryReader, RegistryWriter};
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
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        *self
            .0
            .borrow_mut()
            .iter_mut()
            .find(|v| v.scope == value.scope && same_name(&v.name, &value.name))
            .unwrap() = value.clone();
        Ok(())
    }
}
#[test]
fn history_pages_filter_windows_names_and_explicit_clear_preserves_current_state() {
    let dir = std::env::temp_dir().join(format!(
        "evm-history-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry = Registry(RefCell::new(vec![
        Variable::text(Scope::User, "\u{00c4}Fixture", "initial", false),
        Variable::text(Scope::User, "Other", "keep", false),
    ]));
    let mut app = Application::open(&registry, &dir);
    for (name, value) in [
        ("\u{00c4}Fixture", "one"),
        ("Other", "two"),
        ("\u{00c4}Fixture", "three"),
    ] {
        app.edit_entries(Scope::User, name, vec![value.into()])
            .unwrap();
        app.save(&registry, Scope::User, name).unwrap();
    }
    let baseline = app.baseline().unwrap().to_vec();
    app.backup_history(&dir.join("retained.sqlite")).unwrap();
    let backup = std::fs::read(dir.join("retained.sqlite")).unwrap();
    assert_eq!(
        app.history(Some((Scope::User, "\u{00e4}fixture")), 0, 1)
            .unwrap()[0]
            .after
            .as_ref()
            .unwrap()
            .raw_text()
            .unwrap(),
        "three"
    );
    assert_eq!(
        app.history(Some((Scope::User, "\u{00e4}fixture")), 1, 1)
            .unwrap()[0]
            .after
            .as_ref()
            .unwrap()
            .raw_text()
            .unwrap(),
        "one"
    );
    assert!(app.history(None, 0, 0).unwrap().is_empty());
    assert!(app.history(None, usize::MAX, 10).unwrap().is_empty());
    assert_eq!(app.history(None, 0, 1000).unwrap().len(), 3);
    app.edit_entries(Scope::User, "Other", vec!["unsaved".into()])
        .unwrap();
    assert_eq!(
        app.clear_history(Some((Scope::User, "\u{00e4}fixture")))
            .unwrap(),
        2
    );
    assert_eq!(app.history(None, 0, 50).unwrap().len(), 1);
    assert_eq!(app.baseline().unwrap(), baseline);
    assert_eq!(
        app.draft(Scope::User, "Other").unwrap().entries,
        vec!["unsaved"]
    );
    assert_eq!(registry.0.borrow()[0].raw_text().unwrap(), "three");
    assert_eq!(std::fs::read(dir.join("retained.sqlite")).unwrap(), backup);
    assert_eq!(app.clear_history(None).unwrap(), 1);
    drop(app);
    let reopened = Application::open(&registry, &dir);
    assert!(reopened.history(None, 0, 100).unwrap().is_empty());
    let after_restart = reopened.baseline().unwrap();
    assert_eq!(after_restart.len(), baseline.len());
    assert!(baseline.iter().all(|value| after_restart.contains(value)));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn history_page_requests_are_capped_and_do_not_skip_duplicates() {
    let dir = std::env::temp_dir().join(format!(
        "evm-pages-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry = Registry(RefCell::new(vec![Variable::text(
        Scope::User,
        "Fixture",
        "initial",
        false,
    )]));
    let mut app = Application::open(&registry, &dir);
    for i in 0..105 {
        app.edit_entries(Scope::User, "Fixture", vec![i.to_string()])
            .unwrap();
        app.save(&registry, Scope::User, "Fixture").unwrap();
    }
    let page = app.history(None, 0, usize::MAX).unwrap();
    assert_eq!(page.len(), 100);
    assert_eq!(page[0].after.as_ref().unwrap().raw_text().unwrap(), "104");
    assert_eq!(page[99].after.as_ref().unwrap().raw_text().unwrap(), "5");
    let last = app.history(None, 100, 100).unwrap();
    assert_eq!(last.len(), 5);
    assert_eq!(last[4].after.as_ref().unwrap().raw_text().unwrap(), "0");
    assert!(app.history(None, 105, 100).unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn clear_completed_history_never_removes_pending_recovery_or_a_baseline() {
    struct RecheckDenied {
        inner: Registry,
        reads: std::cell::Cell<usize>,
    }
    impl RegistryReader for RecheckDenied {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            let n = self.reads.get() + 1;
            self.reads.set(n);
            if n == 4 {
                Err("Owned fixture recheck denial".into())
            } else {
                self.inner.read_scope(scope)
            }
        }
    }
    impl RegistryWriter for RecheckDenied {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("Failed recheck must not reach a write")
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "evm-pending-clear-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry = RecheckDenied {
        inner: Registry(RefCell::new(vec![Variable::text(
            Scope::User,
            "Fixture",
            "initial",
            false,
        )])),
        reads: std::cell::Cell::new(0),
    };
    let mut app = Application::open(&registry, &dir);
    let baseline = app.baseline().unwrap().to_vec();
    app.edit_entries(Scope::User, "Fixture", vec!["draft".into()])
        .unwrap();
    assert!(app.save(&registry, Scope::User, "Fixture").is_err());
    let pending = app.pending_operations().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        app.clear_history(Some((Scope::User, "fixture"))).unwrap(),
        0
    );
    assert_eq!(app.clear_history(None).unwrap(), 0);
    assert_eq!(app.pending_operations().unwrap(), pending);
    assert_eq!(app.baseline().unwrap(), baseline);
    assert_eq!(registry.inner.0.borrow()[0].raw_text().unwrap(), "initial");
    let reopened = Application::open(&registry, &dir);
    assert_eq!(reopened.pending_operations().unwrap(), pending);
    let after_restart = reopened.baseline().unwrap();
    assert_eq!(after_restart.len(), baseline.len());
    assert!(baseline.iter().all(|value| after_restart.contains(value)));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn large_snapshot_comparison_keeps_simple_insertion_aligned_with_bounded_work() {
    let before = vec!["same"; 10000].join(";");
    let after = format!("insert;{before}");
    let comparison = record(Some(&before), Some(&after)).comparison();
    assert_eq!(comparison.rows.len(), 10001);
    assert_eq!(comparison.rows[0].change, EntryChange::Added);
    assert!(comparison.rows[1..]
        .iter()
        .all(|r| r.change == EntryChange::Unchanged));
}
