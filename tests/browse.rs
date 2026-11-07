use environment_variable_manager::{Application, RegistryReader, Scope, Variable};
use std::path::PathBuf;

struct Fixture;
impl RegistryReader for Fixture {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(vec![Variable::text(
            scope,
            "Path",
            " a;;b,b;%SystemRoot%; a;",
            false,
        )])
    }
}
fn unreserved_path(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "evm-test-{label}-{}-{}-{}",
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
    assert!(
        !path.exists(),
        "Never reuse a pre-existing test location: {}",
        path.display()
    );
    path
}
fn directory(label: &str) -> PathBuf {
    let path = unreserved_path(label);
    std::fs::create_dir(&path).expect("Exclusively reserve owned test directory");
    path
}
#[test]
fn browse_keeps_user_and_system_variables_distinct() {
    let path = directory("browse");
    let app = Application::open(&Fixture, &path);
    let variables = app.browse("", None);
    assert_eq!(
        variables
            .iter()
            .map(|v| (v.scope, v.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(Scope::User, "Path"), (Scope::System, "Path")]
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn selection_preserves_exact_entries_and_searches_name_or_raw_value() {
    let path = directory("entries");
    let app = Application::open(&Fixture, &path);
    let selected = app
        .select(Scope::User, "pAtH")
        .expect("case-insensitive identity");
    assert_eq!(
        selected.entries().unwrap(),
        vec![" a", "", "b,b", "%SystemRoot%", " a", ""]
    );
    assert_eq!(app.browse("systemroot", Some(Scope::System)).len(), 1);
    assert_eq!(app.browse("missing", None).len(), 0);
    std::fs::remove_dir_all(path).unwrap();
}

struct Unreadable;
impl RegistryReader for Unreadable {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        match scope {
            Scope::User => Ok(vec![]),
            Scope::System => Err("Access denied (5)".into()),
        }
    }
}
#[test]
fn unreadable_scope_is_not_an_empty_scope() {
    let path = directory("unreadable");
    let app = Application::open(&Unreadable, &path);
    assert_eq!(app.scope_error(Scope::User), None);
    assert_eq!(app.scope_error(Scope::System), Some("Access denied (5)"));
    std::fs::remove_dir_all(path).unwrap();
}

struct Empty;
impl RegistryReader for Empty {
    fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
        Ok(vec![])
    }
}
#[test]
fn first_launch_persists_baseline_and_reopen_records_gap_before_advancing_it() {
    let path = unreserved_path("baseline");
    let first = Application::open(&Fixture, &path);
    assert_eq!(first.storage_error(), None);
    assert_eq!(first.baseline().unwrap().len(), 2);
    drop(first);
    let reopened = Application::open(&Empty, &path);
    assert!(reopened.browse("", None).is_empty());
    assert_eq!(
        reopened.history(None, 0, 10).unwrap()[0]
            .before
            .as_ref()
            .unwrap()
            .entries()
            .unwrap(),
        vec![" a", "", "b,b", "%SystemRoot%", " a", ""]
    );
    assert!(reopened.baseline().unwrap().is_empty());
    assert_eq!(reopened.history(None, 0, 10).unwrap().len(), 2);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn corrupt_existing_database_preserves_bytes_and_leaves_browsing_available() {
    let path = directory("corrupt");
    std::fs::create_dir_all(&path).unwrap();
    let database = path.join("environment.sqlite");
    std::fs::write(&database, b"preserve this existing data").unwrap();
    let app = Application::open(&Fixture, &path);
    assert!(app.storage_error().is_some());
    assert_eq!(app.browse("", None).len(), 2);
    assert_eq!(
        std::fs::read(&database).unwrap(),
        b"preserve this existing data"
    );
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn read_failure_never_initializes_an_incomplete_baseline() {
    let path = directory("no-baseline");
    let unreadable = Application::open(&Unreadable, &path);
    assert!(unreadable.baseline().is_none());
    drop(unreadable);
    let recovered = Application::open(&Fixture, &path);
    assert_eq!(recovered.baseline().unwrap().len(), 2);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn unavailable_data_directory_does_not_hide_current_variables() {
    let path = unreserved_path("file-instead-of-directory");
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"keep").unwrap();
    drop(file);
    let app = Application::open(&Fixture, &path);
    assert!(app.storage_error().is_some());
    assert_eq!(app.browse("", None).len(), 2);
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unnamed_registry_value_is_read_only_without_losing_its_empty_text() {
    struct Unnamed;
    impl RegistryReader for Unnamed {
        fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
            Ok(vec![Variable::text(scope, "", "", false)])
        }
    }
    let path = directory("unnamed");
    let app = Application::open(&Unnamed, &path);
    let value = app.select(Scope::User, "").unwrap();
    assert!(
        value.read_only(),
        "An unnamed registry value is not an editable environment-variable name"
    );
    assert_eq!(value.entries().unwrap(), vec![""]);
    std::fs::remove_dir_all(path).unwrap();
}
