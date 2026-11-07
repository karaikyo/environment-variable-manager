//! Developer-only linked-library and old-database compatibility diagnostic.
//! Never included in the portable product; never writes Windows variables.
use environment_variable_manager::{Application, RegistryReader, Scope, Variable};
use std::path::Path;
struct Unreadable;
impl RegistryReader for Unreadable {
    fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
        Err("Diagnostic: read stored baseline without observing Windows".into())
    }
}
struct Snapshot(Vec<Variable>);
impl RegistryReader for Snapshot {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(self
            .0
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
fn history(app: &Application) -> Vec<environment_variable_manager::HistoryRecord> {
    let mut result = Vec::new();
    loop {
        let page = app.history(None, result.len(), 100).unwrap();
        if page.is_empty() {
            return result;
        }
        result.extend(page);
    }
}
fn compatibility(source: &Path) {
    let original = std::fs::read(source).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "evm-sqlite-upgrade-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("environment.sqlite"), &original).unwrap();
    let app = Application::open(&Unreadable, &directory);
    assert_eq!(app.storage_error(), None);
    let snapshot = Snapshot(app.baseline().unwrap().to_vec());
    let expected_history = history(&app);
    let expected_pending = app.pending_operations().unwrap();
    drop(app);
    let reopened = Application::open(&snapshot, &directory);
    assert_eq!(reopened.storage_error(), None);
    assert_eq!(reopened.baseline().unwrap(), snapshot.0);
    assert_eq!(history(&reopened), expected_history);
    assert_eq!(reopened.pending_operations().unwrap(), expected_pending);
    let backup_directory = directory.join("backup");
    std::fs::create_dir(&backup_directory).unwrap();
    reopened
        .backup_history(&backup_directory.join("environment.sqlite"))
        .unwrap();
    let backup = Application::open(&snapshot, &backup_directory);
    assert_eq!(backup.storage_error(), None);
    assert_eq!(history(&backup), expected_history);
    assert_eq!(backup.baseline().unwrap(), snapshot.0);
    assert_eq!(backup.pending_operations().unwrap(), expected_pending);
    assert_eq!(
        std::fs::read(source).unwrap(),
        original,
        "Original fixture must remain untouched"
    );
    println!("Compatible: {} baseline values, {} completed history records, {} pending intents; source unchanged; backup verified", snapshot.0.len(), expected_history.len(), expected_pending.len());
    println!("Evidence directory: {}", directory.display());
}
fn main() {
    println!(
        "SQLite {} ({})",
        rusqlite::version(),
        rusqlite::version_number()
    );
    assert_eq!(rusqlite::version(), "3.54.1");
    assert_eq!(rusqlite::version_number(), 3_054_001);
    if let Some(source) = std::env::args_os().nth(1) {
        compatibility(Path::new(&source));
    }
}
