use environment_variable_manager::{
    Application, RegistryReader, RegistryWriter, Scope, Variable, WriteOutcome,
};
use std::cell::RefCell;
struct Registry {
    values: RefCell<Vec<Variable>>,
    response: RefCell<Option<WriteOutcome>>,
    during_consent: RefCell<Option<Variable>>,
}
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
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
    fn write_user(&self, _: &Variable) -> Result<(), String> {
        panic!("System operation must never use user writer")
    }
    fn change_system(
        &self,
        name: &str,
        _: Option<&Variable>,
        desired: Option<&Variable>,
    ) -> WriteOutcome {
        if let Some(external) = self.during_consent.borrow_mut().take() {
            self.values
                .borrow_mut()
                .retain(|v| !environment_variable_manager::same_name(&v.name, name));
            self.values.borrow_mut().push(external);
        }
        if let Some(outcome) = self.response.borrow_mut().take() {
            return outcome;
        }
        self.values
            .borrow_mut()
            .retain(|v| !environment_variable_manager::same_name(&v.name, name));
        self.values.borrow_mut().extend(desired.cloned());
        WriteOutcome::Attempted(Ok(()))
    }
}
fn fixture() -> (Registry, std::path::PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-system-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).expect("Exclusively reserve owned test data; abort on collision");
    eprintln!("Owned system fixture: {}", path.display());
    (
        Registry {
            values: RefCell::new(vec![Variable::text(Scope::System, "Owned", "before", true)]),
            response: RefCell::new(None),
            during_consent: RefCell::new(None),
        },
        path,
    )
}

#[test]
fn system_edit_uses_common_history_and_cancel_preserves_draft_without_app_edit() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(
        Scope::System,
        "Owned",
        vec!["%Root%".into(), "".into(), "line\ninside".into()],
    )
    .unwrap();
    *registry.response.borrow_mut() = Some(WriteOutcome::Cancelled);
    assert!(app
        .save(&registry, Scope::System, "Owned")
        .unwrap_err()
        .contains("cancelled"));
    assert!(app.draft(Scope::System, "Owned").is_some());
    assert!(app.pending_operations().unwrap().is_empty());
    assert!(app.history(None, 0, 50).unwrap().is_empty());
    assert!(app.can_save(Scope::System, "Owned"));
    app.save(&registry, Scope::System, "Owned").unwrap();
    let history = app.history(None, 0, 50).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].after,
        Some(Variable::text(
            Scope::System,
            "Owned",
            "%Root%;;line\ninside",
            true
        ))
    );
    assert_eq!(history[0].source, "app-made");
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn system_create_and_delete_share_absence_empty_and_confirmation_semantics() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.create_system("New").unwrap();
    app.save(&registry, Scope::System, "New").unwrap();
    assert_eq!(
        app.select(Scope::System, "New")
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("")
    );
    app.request_delete(Scope::System, "New").unwrap();
    app.confirm_delete(&registry).unwrap();
    assert!(app.select(Scope::System, "New").is_none());
    let records = app.history(Some((Scope::System, "New")), 0, 50).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].after, None);
    assert_eq!(records[1].before, None);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn system_conflict_after_consent_is_sticky_and_never_app_attribution() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::System, "Owned", vec!["desired".into()])
        .unwrap();
    *registry.response.borrow_mut() = Some(WriteOutcome::Conflict);
    assert!(app
        .save(&registry, Scope::System, "Owned")
        .unwrap_err()
        .contains("Conflict"));
    app.refresh(&registry);
    assert!(!app.can_save(Scope::System, "Owned"));
    assert!(app.history(None, 0, 50).unwrap().is_empty());
    assert!(app.pending_operations().unwrap().is_empty());
    app.discard(&registry, Scope::System, "Owned").unwrap();
    app.edit_entries(Scope::System, "Owned", vec!["desired".into()])
        .unwrap();
    assert!(app.can_save(Scope::System, "Owned"));
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn system_lost_response_retains_intent_and_recovers_unknown_actor_without_replay() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.edit_entries(Scope::System, "Owned", vec!["desired".into()])
        .unwrap();
    *registry.response.borrow_mut() = Some(WriteOutcome::Uncertain);
    let error = app.save(&registry, Scope::System, "Owned").unwrap_err();
    assert!(
        error.contains("Uncertain"),
        "Unexpected public result: {error}; owned fixture: {}",
        path.display()
    );
    assert_eq!(app.pending_operations().unwrap().len(), 1);
    assert!(app.storage_error().is_some());
    registry.values.borrow_mut()[0] =
        Variable::text(Scope::System, "Owned", "later external", true);
    app.refresh(&registry);
    let records = app.history(None, 0, 50).unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].source.contains("actor unknown"));
    assert_eq!(records[0].outcome, "uncertain");
    assert_eq!(
        registry.values.borrow()[0].raw_text().as_deref(),
        Some("later external")
    );
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn system_delete_conflict_cannot_be_retried_without_discard_even_if_value_returns() {
    let (registry, path) = fixture();
    let mut app = Application::open(&registry, &path);
    app.request_delete(Scope::System, "Owned").unwrap();
    *registry.response.borrow_mut() = Some(WriteOutcome::Conflict);
    {
        let error = app.confirm_delete(&registry).unwrap_err();
        assert!(error.contains("Conflict"), "Expected conflict, got {error}");
    }
    app.refresh(&registry);
    {
        let error = app.confirm_delete(&registry).unwrap_err();
        assert!(error.contains("Conflict"), "Expected conflict, got {error}");
    }
    app.discard(&registry, Scope::System, "Owned").unwrap();
    app.request_delete(Scope::System, "Owned").unwrap();
    app.confirm_delete(&registry).unwrap();
    assert!(registry.values.borrow().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn system_prewrite_refusals_never_attribute_an_external_consent_interval_change_to_the_app() {
    for refusal in [
        WriteOutcome::Cancelled,
        WriteOutcome::Rejected("wrong account".into()),
        WriteOutcome::Conflict,
    ] {
        let (registry, path) = fixture();
        let mut app = Application::open(&registry, &path);
        app.edit_entries(Scope::System, "Owned", vec!["desired".into()])
            .unwrap();
        *registry.during_consent.borrow_mut() =
            Some(Variable::text(Scope::System, "Owned", "desired", true));
        *registry.response.borrow_mut() = Some(refusal);
        assert!(app.save(&registry, Scope::System, "Owned").is_err());
        let history = app.history(None, 0, 50).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].source, "external (actor unknown)");
        assert_eq!(history[0].outcome, "observed");
        assert_eq!(
            history[0].after.as_ref().unwrap().raw_text().as_deref(),
            Some("desired")
        );
        assert!(app.pending_operations().unwrap().is_empty());
        assert!(app.draft(Scope::System, "Owned").unwrap().conflict);
        assert!(app.storage_error().is_none());
        app.refresh(&registry);
        assert_eq!(app.history(None, 0, 50).unwrap().len(), 1);
        std::fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn system_conflict_at_final_prelaunch_recheck_is_external_not_an_app_edit() {
    struct Changed {
        registry: Registry,
        reads: std::cell::Cell<u32>,
    }
    impl RegistryReader for Changed {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            if scope == Scope::System {
                self.reads.set(self.reads.get() + 1);
                if self.reads.get() == 3 {
                    self.registry.values.borrow_mut()[0] =
                        Variable::text(Scope::System, "Owned", "external", true);
                }
            }
            self.registry.read_scope(scope)
        }
    }
    impl RegistryWriter for Changed {
        fn write_user(&self, _: &Variable) -> Result<(), String> {
            panic!("No write permitted")
        }
        fn change_system(
            &self,
            _: &str,
            _: Option<&Variable>,
            _: Option<&Variable>,
        ) -> WriteOutcome {
            panic!("A failed final recheck must never launch consent")
        }
    }
    let (registry, path) = fixture();
    let changed = Changed {
        registry,
        reads: std::cell::Cell::new(0),
    };
    let mut app = Application::open(&changed, &path);
    app.edit_entries(Scope::System, "Owned", vec!["desired".into()])
        .unwrap();
    assert!(app
        .save(&changed, Scope::System, "Owned")
        .unwrap_err()
        .contains("Conflict"));
    let records = app.history(None, 0, 50).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, "external (actor unknown)");
    assert!(app.pending_operations().unwrap().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}
