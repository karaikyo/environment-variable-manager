//! Persistent command-line writer proof: only collision-guarded variables created by this test.
use environment_variable_manager::{
    windows_registry::WindowsRegistry, Application, RegistryReader, Scope, Variable,
};
use std::{process::Command, time::Duration};
struct OwnedVariable {
    registry: WindowsRegistry,
    name: String,
    expected: Option<Variable>,
}
impl OwnedVariable {
    fn current(&self) -> Option<Variable> {
        self.registry
            .read_scope(Scope::User)
            .unwrap()
            .into_iter()
            .find(|v| environment_variable_manager::same_name(&v.name, &self.name))
    }
    fn write(&mut self, text: &str, expand: bool) {
        assert_eq!(
            self.current(),
            self.expected,
            "Fixture ownership changed; refusing overwrite"
        );
        let output = Command::new("reg.exe")
            .args([
                "add",
                "HKCU\\Environment",
                "/v",
                &self.name,
                "/t",
                if expand { "REG_EXPAND_SZ" } else { "REG_SZ" },
                "/d",
                text,
                "/f",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Persistent command-line writer failed"
        );
        let value = Variable::text(Scope::User, &self.name, text, expand);
        assert_eq!(self.current(), Some(value.clone()));
        self.expected = Some(value);
    }
    fn delete(&mut self) {
        assert_eq!(
            self.current(),
            self.expected,
            "Fixture ownership changed; refusing delete"
        );
        assert!(self.expected.is_some());
        assert!(Command::new("reg.exe")
            .args(["delete", "HKCU\\Environment", "/v", &self.name, "/f"])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(self.current(), None);
        self.expected = None;
    }
}
impl Drop for OwnedVariable {
    fn drop(&mut self) {
        if self.expected.is_some() && self.current() == self.expected {
            let _ = Command::new("reg.exe")
                .args(["delete", "HKCU\\Environment", "/v", &self.name, "/f"])
                .output();
        }
    }
}
#[test]
fn persistent_cli_writes_and_app_results_are_observed_without_duplicate_attribution() {
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut owned = OwnedVariable {
        registry: WindowsRegistry::environment(),
        name: format!("EVM_OBSERVE_TEST_{suffix}"),
        expected: None,
    };
    assert_eq!(owned.current(), None, "Abort on name collision");
    let directory = std::env::temp_dir().join(format!("evm-cli-{suffix}"));
    assert!(!directory.exists());
    let mut watch = owned.registry.watch();
    assert!(watch.poll(Duration::ZERO).errors.is_empty());
    let mut app = Application::open(&owned.registry, &directory);
    owned.write("first", false);
    assert!(watch.poll(Duration::from_secs(2)).changed);
    app.observe(&owned.registry);
    app.edit_entries(Scope::User, &owned.name, vec!["app edit".into()])
        .unwrap();
    app.save(&owned.registry, Scope::User, &owned.name).unwrap();
    owned.expected = Some(Variable::text(Scope::User, &owned.name, "app edit", false));
    assert!(watch.poll(Duration::from_secs(2)).changed);
    app.observe(&owned.registry);
    app.refresh(&owned.registry);
    let records = app
        .history(Some((Scope::User, &owned.name)), 0, 100)
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].source, "app-made");
    assert_eq!(records[1].source, "external (actor unknown)");
    owned.write("", true);
    assert!(watch.poll(Duration::from_secs(2)).changed);
    app.observe(&owned.registry);
    assert_eq!(
        app.select(Scope::User, &owned.name)
            .unwrap()
            .raw_text()
            .as_deref(),
        Some("")
    );
    assert_eq!(
        app.select(Scope::User, &owned.name).unwrap().registry_type,
        2
    );
    owned.delete();
    assert!(watch.poll(Duration::from_secs(2)).changed);
    app.observe(&owned.registry);
    let records = app
        .history(Some((Scope::User, &owned.name)), 0, 100)
        .unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].after, None);
    assert_eq!(
        records[1].after.as_ref().unwrap().raw_text().as_deref(),
        Some("")
    );
    assert_eq!(records.iter().filter(|r| r.source == "app-made").count(), 1);
    drop(app);
    drop(watch);
    std::fs::remove_dir_all(directory).unwrap();
}
