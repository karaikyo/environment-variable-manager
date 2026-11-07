//! Versioned upgrades of the original database. No replacement or fallback path.
use rusqlite::Connection;
use std::path::Path;
pub(super) const VERSION: i32 = 2;
pub(super) const OPERATIONS: &str = "CREATE TABLE operations(id INTEGER PRIMARY KEY,scope INTEGER NOT NULL,name TEXT NOT NULL,before_type INTEGER,before_raw BLOB,desired_type INTEGER,desired_raw BLOB,after_type INTEGER,after_raw BLOB,time_ms INTEGER NOT NULL,source TEXT NOT NULL,outcome TEXT NOT NULL);";
pub(super) const SETTINGS: &str = "CREATE TABLE settings(id INTEGER PRIMARY KEY CHECK(id=1), startup_enabled INTEGER NOT NULL CHECK(startup_enabled IN (0,1)), startup_command TEXT);";
fn error(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(message.into()),
    )
}
pub(super) fn check(db: &Connection) -> rusqlite::Result<i32> {
    let id: i32 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if id != super::APPLICATION_ID || !(1..=VERSION).contains(&version) {
        return Err(error(
            "Unsupported data format or newer schema; existing data preserved",
        ));
    }
    let journal: String = db.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if journal != "delete" {
        return Err(error(
            "Expected DELETE journaling; original data format preserved",
        ));
    }
    Ok(version)
}
pub(super) fn upgrade(db: &mut Connection, directory: &Path) -> rusqlite::Result<()> {
    if check(db)? == VERSION {
        return Ok(());
    }
    // Every attempt exclusively reserves a new destination. A failed prior attempt
    // (even an incomplete backup) can never block retry or be overwritten.
    let mut suffix = 0u64;
    let backup_path = loop {
        let name = if suffix == 0 {
            "before-schema-1-to-2.sqlite".into()
        } else {
            format!("before-schema-1-to-2-{suffix}.sqlite")
        };
        let path = directory.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                drop(file);
                break path;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
            Err(e) => {
                return Err(error(format!(
                    "Migration backup unavailable; original preserved: {e}"
                )))
            }
        }
    };
    let mut backup = Connection::open(&backup_path)?;
    super::copy_database(db, &mut backup)?;
    let integrity: String = backup.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(error(
            "Migration backup integrity failed; original preserved",
        ));
    }
    drop(backup);
    // All format changes and the version advance commit together. Any error rolls
    // back to the original database; the verified pre-migration copy is retained.
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (table, sql) in [("operations", OPERATIONS), ("settings", SETTINGS)] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
            [table],
            |r| r.get(0),
        )?;
        if !exists {
            tx.execute_batch(sql)?;
        }
    }
    // Reject incompatible legacy extensions instead of stamping an unusable format.
    tx.prepare("SELECT scope,name,registry_type,raw FROM baseline LIMIT 0")?;
    tx.prepare("SELECT initialized FROM state WHERE id=1")?;
    tx.prepare("SELECT id,scope,name,before_type,before_raw,desired_type,desired_raw,after_type,after_raw,time_ms,source,outcome FROM operations LIMIT 0")?;
    tx.prepare("SELECT id,startup_enabled,startup_command FROM settings LIMIT 0")?;
    tx.execute_batch("PRAGMA user_version=2;")?;
    let integrity: String = tx.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(error("Migration integrity failed; original preserved"));
    }
    tx.commit()
}
