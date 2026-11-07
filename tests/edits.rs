use environment_variable_manager::{
    Application, RegistryReader, RegistryWriter, SaveOutcome, Scope, Variable,
};
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
        let mut values = self.0.borrow_mut();
        let existing = values
            .iter_mut()
            .find(|v| {
                v.scope == value.scope
                    && environment_variable_manager::same_name(&v.name, &value.name)
            })
            .unwrap();
        *existing = value.clone();
        Ok(())
    }
}
fn fixture() -> (Registry, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "evm-edits-{}-{}-{}",
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
        Registry(RefCell::new(vec![
            Variable::text(Scope::User, "MiXeD", " a;;b\nline;", true),
            Variable::text(Scope::User, "Other", "keep", false),
        ])),
        directory,
    )
}
#[test]
fn save_round_trips_exact_type_spelling_and_history_after_restart() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(
        Scope::User,
        "mixed",
        vec![
            "%HOME%".into(),
            "".into(),
            "x\n\ry".into(),
            "x".into(),
            "x".into(),
            "".into(),
        ],
    )
    .unwrap();
    assert_eq!(
        app.save(&registry, Scope::User, "mixed"),
        Ok(SaveOutcome::Saved)
    );
    let expected = Variable::text(Scope::User, "MiXeD", "%HOME%;;x\n\ry;x;x;", true);
    assert_eq!(app.select(Scope::User, "mixed"), Some(&expected));
    assert!(
        app.baseline().unwrap().contains(&expected),
        "Saved result becomes the observed baseline immediately"
    );
    assert!(app.draft(Scope::User, "mixed").is_none());
    drop(app);
    let reopened = Application::open(&registry, &directory);
    let history = reopened
        .history(Some((Scope::User, "mixed")), 0, 50)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].before,
        Some(Variable::text(Scope::User, "MiXeD", " a;;b\nline;", true))
    );
    assert_eq!(history[0].after, Some(expected));
    assert_eq!(history[0].source, "app-made");
    assert_eq!(history[0].outcome, "saved");
    assert!(history[0].time_ms > 0);
    assert!(reopened.pending_operations().unwrap().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn no_op_invalid_entries_and_stale_drafts_make_no_writes_or_history() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(
        Scope::User,
        "mixed",
        vec![" a".into(), "".into(), "b\nline".into(), "".into()],
    )
    .unwrap();
    assert_eq!(
        app.save(&registry, Scope::User, "mixed"),
        Ok(SaveOutcome::NoChange)
    );
    app.edit_entries(Scope::User, "mixed", vec!["bad;entry".into()])
        .unwrap();
    assert!(app.save(&registry, Scope::User, "mixed").is_err());
    app.edit_entries(Scope::User, "mixed", vec!["draft".into()])
        .unwrap();
    registry.0.borrow_mut()[0] = Variable::text(Scope::User, "MiXeD", "external", true);
    assert!(app
        .save(&registry, Scope::User, "mixed")
        .unwrap_err()
        .contains("Conflict"));
    assert_eq!(registry.0.borrow()[0].raw_text().unwrap(), "external");
    assert_eq!(
        app.draft(Scope::User, "mixed").unwrap().entries,
        vec!["draft"]
    );
    assert!(app.history(None, 0, 50).unwrap().is_empty());
    assert!(!app.can_save(Scope::User, "mixed"));
    registry.0.borrow_mut()[0] = Variable::text(Scope::User, "MiXeD", " a;;b\nline;", true);
    assert!(
        app.save(&registry, Scope::User, "mixed")
            .unwrap_err()
            .contains("Conflict"),
        "A previously detected conflict requires explicit reload"
    );
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn drafts_keep_exact_entries_across_navigation_and_discard_only_selected() {
    let (registry, directory) = fixture();
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(
        Scope::User,
        "mixed",
        vec!["".into(), "b\nline".into(), " a".into(), "".into()],
    )
    .unwrap();
    app.edit_entries(Scope::User, "Other", vec!["draft".into()])
        .unwrap();
    assert!(app.browse("missing", None).is_empty());
    assert_eq!(
        app.draft(Scope::User, "MIXED").unwrap().entries,
        vec!["", "b\nline", " a", ""]
    );
    registry.0.borrow_mut()[0] = Variable::text(Scope::User, "MiXeD", "current", true);
    app.discard(&registry, Scope::User, "mixed").unwrap();
    assert_eq!(
        app.select(Scope::User, "mixed")
            .unwrap()
            .raw_text()
            .unwrap(),
        "current"
    );
    assert!(app.draft(Scope::User, "mixed").is_none());
    assert_eq!(
        app.draft(Scope::User, "Other").unwrap().entries,
        vec!["draft"]
    );
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn failed_recheck_preserves_pending_intent_and_blocks_more_writes() {
    struct ReadDenied {
        inner: Registry,
        reads: std::cell::Cell<usize>,
    }
    impl RegistryReader for ReadDenied {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            let count = self.reads.get() + 1;
            self.reads.set(count);
            if count == 4 {
                Err("Recheck denied".into())
            } else {
                self.inner.read_scope(scope)
            }
        }
    }
    impl RegistryWriter for ReadDenied {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("A failed recheck must never write")
        }
    }
    let (inner, directory) = fixture();
    let registry = ReadDenied {
        inner,
        reads: std::cell::Cell::new(0),
    };
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "mixed", vec!["draft".into()])
        .unwrap();
    assert!(app.save(&registry, Scope::User, "mixed").is_err());
    assert_eq!(
        app.pending_operations().unwrap()[0]
            .desired_after
            .as_ref()
            .unwrap()
            .raw_text()
            .unwrap(),
        "draft"
    );
    assert!(!app.can_save(Scope::User, "mixed"));
    drop(app);
    assert!(Application::open(&registry, &directory)
        .storage_error()
        .is_some());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn unavailable_intent_storage_never_reaches_windows_write() {
    struct NoWrite(Registry);
    impl RegistryReader for NoWrite {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            self.0.read_scope(scope)
        }
    }
    impl RegistryWriter for NoWrite {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("Unsaved intent must not reach Windows")
        }
    }
    let (registry, directory) = fixture();
    let registry = NoWrite(registry);
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "mixed", vec!["draft".into()])
        .unwrap();
    let database = directory.join("environment.sqlite");
    let held = directory.join("held.sqlite");
    std::fs::rename(&database, &held).unwrap();
    std::fs::create_dir(&database).unwrap();
    assert!(app.save(&registry, Scope::User, "mixed").is_err());
    assert_eq!(
        registry.0 .0.borrow()[0].raw_text().unwrap(),
        " a;;b\nline;"
    );
    assert!(!app.can_save(Scope::User, "mixed"));
    app.discard(&registry, Scope::User, "mixed").unwrap();
    assert!(
        app.draft(Scope::User, "mixed").is_none(),
        "Discard remains available without recording"
    );
    std::fs::remove_dir(&database).unwrap();
    std::fs::rename(&held, &database).unwrap();
    assert!(Application::open(&registry, &directory)
        .history(None, 0, 50)
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn recording_failure_after_write_reports_applied_uncertain_and_retains_intent() {
    struct ResultUnavailable {
        inner: Registry,
        directory: PathBuf,
    }
    impl RegistryReader for ResultUnavailable {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            self.inner.read_scope(scope)
        }
    }
    impl RegistryWriter for ResultUnavailable {
        fn write_user(&self, value: &Variable) -> Result<(), String> {
            let pending = Application::open(&self.inner, &self.directory)
                .pending_operations()
                .unwrap();
            assert_eq!(
                pending.len(),
                1,
                "Intent is durable before entering Windows write"
            );
            assert_eq!(
                pending[0]
                    .expected_before
                    .as_ref()
                    .unwrap()
                    .raw_text()
                    .unwrap(),
                " a;;b\nline;"
            );
            assert_eq!(pending[0].desired_after.as_ref(), Some(value));
            self.inner.write_user(value)?;
            std::fs::rename(
                self.directory.join("environment.sqlite"),
                self.directory.join("held.sqlite"),
            )
            .unwrap();
            std::fs::create_dir(self.directory.join("environment.sqlite")).unwrap();
            Ok(())
        }
    }
    let (inner, directory) = fixture();
    let registry = ResultUnavailable {
        inner,
        directory: directory.clone(),
    };
    let mut app = Application::open(&registry, &directory);
    app.edit_entries(Scope::User, "mixed", vec!["written".into()])
        .unwrap();
    assert!(app
        .save(&registry, Scope::User, "mixed")
        .unwrap_err()
        .contains("Applied: Windows currently matches"));
    assert_eq!(registry.inner.0.borrow()[0].raw_text().unwrap(), "written");
    assert!(app.draft(Scope::User, "mixed").is_some());
    assert!(!app.can_save(Scope::User, "mixed"));
    std::fs::remove_dir(directory.join("environment.sqlite")).unwrap();
    std::fs::rename(
        directory.join("held.sqlite"),
        directory.join("environment.sqlite"),
    )
    .unwrap();
    let reopened = Application::open(&registry, &directory);
    assert!(reopened.storage_error().is_some());
    assert_eq!(reopened.pending_operations().unwrap().len(), 1);
    assert!(
        reopened.history(None, 0, 50).unwrap().is_empty(),
        "No invented completed history"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn invalid_persisted_names_remain_exact_and_read_only_in_both_scopes() {
    let (_, directory) = fixture();
    let values = [Scope::User, Scope::System]
        .into_iter()
        .flat_map(|scope| {
            ["A=B".to_owned(), "x".repeat(16384)]
                .into_iter()
                .map(move |name| Variable::text(scope, &name, "raw;%literal%", false))
        })
        .collect::<Vec<_>>();
    let registry = Registry(RefCell::new(values.clone()));
    let mut app = Application::open(&registry, &directory);
    assert_eq!(app.browse("", None).len(), 4);
    for value in &values {
        assert_eq!(app.select(value.scope, &value.name), Some(value));
        assert!(app.select(value.scope, &value.name).unwrap().read_only());
        assert!(app
            .edit_entries(value.scope, &value.name, vec!["changed".into()])
            .is_err());
        assert!(app.request_delete(value.scope, &value.name).is_err());
    }
    assert!(app.pending_operations().unwrap().is_empty());
    assert!(app.history(None, 0, 50).unwrap().is_empty());
    assert_eq!(registry.0.into_inner(), values);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn oversized_joined_entries_are_rejected_before_intent_in_both_scopes() {
    let (_, directory) = fixture();
    let values = [Scope::User, Scope::System]
        .into_iter()
        .map(|scope| Variable::text(scope, "Owned", "original", false))
        .collect::<Vec<_>>();
    let registry = Registry(RefCell::new(values.clone()));
    let mut app = Application::open(&registry, &directory);
    for scope in [Scope::User, Scope::System] {
        app.edit_entries(scope, "Owned", vec!["x".repeat(8_388_607)])
            .unwrap();
        assert!(
            app.can_save(scope, "Owned"),
            "exactly 16 MiB including UTF-16 terminator remains supported"
        );
        app.edit_entries(
            scope,
            "Owned",
            vec!["x".repeat(4_194_304), "y".repeat(4_194_303)],
        )
        .unwrap();
        assert!(
            !app.can_save(scope, "Owned"),
            "count entry separators and terminator in the byte bound"
        );
        assert!(app.save(&registry, scope, "Owned").is_err());
        assert!(app.pending_operations().unwrap().is_empty());
        assert!(app.history(None, 0, 50).unwrap().is_empty());
    }
    assert_eq!(registry.0.into_inner(), values);
    std::fs::remove_dir_all(directory).unwrap();
}
