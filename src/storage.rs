mod migrations;
use crate::{HistoryRecord, PendingOperation, Scope, Variable};
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

const APPLICATION_ID: i32 = 0x45564d31;

/// Open an existing database only after checking its identity and version. New
/// files are reserved exclusively; an existing empty/corrupt file is not new data.
pub(crate) fn baseline(
    directory: &Path,
    current: Option<&[Variable]>,
    allow_create: bool,
) -> Result<Option<Vec<Variable>>, String> {
    if allow_create {
        std::fs::create_dir_all(directory)
            .map_err(|e| format!("Data directory unavailable: {e}"))?;
    }
    let path = directory.join("environment.sqlite");
    if allow_create
        && !path.exists()
        && std::fs::read_dir(directory)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("Recording interrupted: database is missing from an existing data directory; contents preserved".into());
    }
    let created = if allow_create {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                drop(file);
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(format!("Database unavailable: {e}")),
        }
    } else {
        false
    };
    let operation = || -> rusqlite::Result<Option<Vec<Variable>>> {
        let mut db = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(std::time::Duration::from_millis(500))?;
        if db.is_readonly("main")? {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY),
                Some("Recording interrupted: database is read-only".into()),
            ));
        }

        if !created {
            migrations::check(&db)?;
        }
        if created {
            db.execute_batch("PRAGMA journal_mode=DELETE;")?;
        }
        db.execute_batch("PRAGMA synchronous=EXTRA; PRAGMA cache_size=-512; PRAGMA mmap_size=0;")?;
        if created {
            let tx = db.transaction()?;
            tx.execute_batch("CREATE TABLE baseline(scope INTEGER NOT NULL CHECK(scope IN (0,1)), name TEXT NOT NULL, registry_type INTEGER NOT NULL, raw BLOB NOT NULL, PRIMARY KEY(scope,name)); CREATE TABLE state(id INTEGER PRIMARY KEY CHECK(id=1), initialized INTEGER NOT NULL CHECK(initialized IN (0,1))); INSERT INTO state VALUES(1,0); PRAGMA application_id=1163283761; PRAGMA user_version=1;")?;
            tx.execute_batch(migrations::OPERATIONS)?;
            tx.execute_batch(migrations::SETTINGS)?;
            tx.execute_batch("PRAGMA user_version=2;")?;
            tx.commit()?;
        } else {
            migrations::upgrade(&mut db, directory)?;
        }
        let initialized: bool =
            db.query_row("SELECT initialized FROM state WHERE id=1", [], |r| r.get(0))?;
        if !initialized {
            let Some(current) = current else {
                return Ok(None);
            };
            let tx = db.transaction()?;
            for value in current {
                tx.execute(
                    "INSERT INTO baseline(scope,name,registry_type,raw) VALUES(?1,?2,?3,?4)",
                    params![
                        if value.scope == Scope::User { 0 } else { 1 },
                        value.name,
                        value.registry_type,
                        value.bytes
                    ],
                )?;
            }
            tx.execute("UPDATE state SET initialized=1 WHERE id=1", [])?;
            tx.commit()?;
        }
        let mut query =
            db.prepare("SELECT scope,name,registry_type,raw FROM baseline ORDER BY scope,name")?;
        let rows = query.query_map([], |r| {
            Ok(Variable {
                scope: if r.get::<_, i32>(0)? == 0 {
                    Scope::User
                } else {
                    Scope::System
                },
                name: r.get(1)?,
                registry_type: r.get(2)?,
                bytes: r.get(3)?,
            })
        })?;
        Ok(Some(rows.collect::<rusqlite::Result<Vec<_>>>()?))
    };
    operation().map_err(|e| format!("Recording unavailable; existing data preserved: {e}"))
}

fn connection(directory: &Path) -> Result<Connection, String> {
    open_connection(directory, false)
}
fn read_connection(directory: &Path) -> Result<Connection, String> {
    open_connection(directory, true)
}
fn open_connection(directory: &Path, read_only: bool) -> Result<Connection, String> {
    let db = Connection::open_with_flags(
        directory.join("environment.sqlite"),
        (if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        }) | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|e| e.to_string())?;
    let identity: (i32, i32) = (
        db.query_row("PRAGMA application_id", [], |r| r.get(0))
            .map_err(|e| e.to_string())?,
        db.query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?,
    );
    if identity != (APPLICATION_ID, migrations::VERSION) {
        return Err("Unsupported data format; existing data preserved".into());
    }
    db.execute_batch("PRAGMA synchronous=EXTRA; PRAGMA cache_size=-512; PRAGMA mmap_size=0;")
        .map_err(|e| e.to_string())?;
    if !read_only {
        let journal: String = db
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if journal != "delete" {
            return Err(
                "Recording interrupted: expected DELETE journaling; existing database preserved"
                    .into(),
            );
        }
    }
    db.create_collation("WINDOWS_ORDINAL", |a, b| {
        if crate::same_name(a, b) {
            std::cmp::Ordering::Equal
        } else {
            let a: Vec<u16> = a.encode_utf16().collect();
            let b: Vec<u16> = b.encode_utf16().collect();
            match unsafe {
                windows_sys::Win32::Globalization::CompareStringOrdinal(
                    a.as_ptr(),
                    a.len() as i32,
                    b.as_ptr(),
                    b.len() as i32,
                    1,
                )
            } {
                1 => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Greater,
            }
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(db)
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub(crate) fn pending(directory: &Path) -> Result<Vec<PendingOperation>, String> {
    let db = read_connection(directory)?;
    let mut query = db.prepare("SELECT id,scope,name,before_type,before_raw,desired_type,desired_raw FROM operations WHERE outcome='pending' ORDER BY id").map_err(|e|e.to_string())?;
    let rows = query
        .query_map([], |r| {
            let scope = if r.get::<_, i32>(1)? == 0 {
                Scope::User
            } else {
                Scope::System
            };
            let name: String = r.get(2)?;
            let expected_before = match r.get::<_, Option<u32>>(3)? {
                Some(registry_type) => Some(Variable {
                    scope,
                    name: name.clone(),
                    registry_type,
                    bytes: r.get(4)?,
                }),
                None => None,
            };
            let desired_after = match r.get::<_, Option<u32>>(5)? {
                Some(registry_type) => Some(Variable {
                    scope,
                    name: name.clone(),
                    registry_type,
                    bytes: r.get(6)?,
                }),
                None => None,
            };
            Ok(PendingOperation {
                id: r.get(0)?,
                scope,
                name,
                expected_before,
                desired_after,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}
pub(crate) fn intent(
    directory: &Path,
    scope: Scope,
    name: &str,
    before: Option<&Variable>,
    desired: Option<&Variable>,
) -> Result<i64, String> {
    let db = connection(directory)?;
    db.execute("INSERT INTO operations(scope,name,before_type,before_raw,desired_type,desired_raw,time_ms,source,outcome) VALUES(?1,?2,?3,?4,?5,?6,?7,'app-made','pending')", params![if scope == Scope::User {0} else {1}, name, before.map(|v|v.registry_type), before.map(|v|&v.bytes), desired.map(|v|v.registry_type), desired.map(|v|&v.bytes), now()]).map_err(|e| format!("Pending intent could not be saved: {e}"))?;
    Ok(db.last_insert_rowid())
}
/// A helper proved that it did not write. Remove only this unattempted intent;
/// external changes are independently observed, never attributed to the app.
pub(crate) fn abandon(directory: &Path, id: i64) -> Result<(), String> {
    let db = connection(directory)?;
    db.execute(
        "DELETE FROM operations WHERE id=?1 AND outcome='pending'",
        [id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn finish(
    directory: &Path,
    id: i64,
    after: Option<&Variable>,
    outcome: &str,
) -> Result<(), String> {
    let mut db = connection(directory)?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute("UPDATE operations SET after_type=?2,after_raw=?3,time_ms=?4,outcome=?5 WHERE id=?1 AND outcome='pending'", params![id, after.map(|v| v.registry_type), after.map(|v| &v.bytes), now(), outcome]).map_err(|e| e.to_string())?;
    if matches!(outcome, "saved" | "applied" | "not-applied" | "uncertain") {
        let (scope, name): (i32, String) = tx
            .query_row("SELECT scope,name FROM operations WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|e| e.to_string())?;
        replace_baseline(&tx, scope, &name, after)?;
    }
    tx.commit().map_err(|e| e.to_string())
}
pub(crate) fn records(
    directory: &Path,
    pending: bool,
    selected: Option<(Scope, &str)>,
    offset: usize,
    limit: usize,
) -> Result<Vec<HistoryRecord>, String> {
    let db = read_connection(directory)?;
    if limit == 0 || offset > i64::MAX as usize {
        return Ok(Vec::new());
    }
    let mut query = db.prepare("SELECT id,scope,name,before_type,before_raw,after_type,after_raw,time_ms,source,outcome FROM operations WHERE (outcome='pending')=?1 AND (?2 IS NULL OR (scope=?2 AND name=?3 COLLATE WINDOWS_ORDINAL)) ORDER BY id DESC LIMIT ?4 OFFSET ?5").map_err(|e| e.to_string())?;
    let rows = query
        .query_map(
            params![
                pending,
                selected.map(|(s, _)| if s == Scope::User { 0 } else { 1 }),
                selected.map(|(_, n)| n),
                limit.min(100) as i64,
                offset as i64
            ],
            |r| {
                let scope = if r.get::<_, i32>(1)? == 0 {
                    Scope::User
                } else {
                    Scope::System
                };
                let name: String = r.get(2)?;
                let after_type: Option<u32> = r.get(5)?;
                Ok(HistoryRecord {
                    id: r.get(0)?,
                    scope,
                    name: name.clone(),
                    before: match r.get::<_, Option<u32>>(3)? {
                        Some(registry_type) => Some(Variable {
                            scope,
                            name: name.clone(),
                            registry_type,
                            bytes: r.get(4)?,
                        }),
                        None => None,
                    },
                    after: match after_type {
                        Some(registry_type) => Some(Variable {
                            scope,
                            name,
                            registry_type,
                            bytes: r.get(6)?,
                        }),
                        None => None,
                    },
                    time_ms: r.get(7)?,
                    source: r.get(8)?,
                    outcome: r.get(9)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}
/// Only completed records are removed. Baselines, intents and backups are separate.
pub(crate) fn clear_records(
    directory: &Path,
    selected: Option<(Scope, &str)>,
) -> Result<usize, String> {
    let db = connection(directory)?;
    db.execute("DELETE FROM operations WHERE outcome<>'pending' AND (?1 IS NULL OR (scope=?1 AND name=?2 COLLATE WINDOWS_ORDINAL))",params![selected.map(|(s,_)|if s==Scope::User{0}else{1}),selected.map(|(_,n)|n)]).map_err(|e|format!("History was not cleared: {e}"))
}

/// Record net differences and advance one successfully read scope atomically.
pub(crate) fn observation(
    directory: &Path,
    scope: Scope,
    previous: &[Variable],
    current: &[Variable],
    gap: bool,
) -> Result<(), String> {
    let mut changes = Vec::new();
    for before in previous {
        let after = current
            .iter()
            .find(|v| crate::same_name(&before.name, &v.name));
        if after != Some(before) {
            changes.push((before.name.as_str(), Some(before), after));
        }
    }
    for after in current {
        if !previous
            .iter()
            .any(|v| crate::same_name(&after.name, &v.name))
        {
            changes.push((after.name.as_str(), None, Some(after)));
        }
    }
    let mut db = connection(directory)?;
    if changes.is_empty() {
        return Ok(());
    }
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let scope_id = if scope == Scope::User { 0 } else { 1 };
    let time = now();
    for (name, before, after) in changes {
        tx.execute("INSERT INTO operations(scope,name,before_type,before_raw,after_type,after_raw,time_ms,source,outcome) VALUES(?1,?2,?3,?4,?5,?6,?7,'external (actor unknown)',?8)", params![scope_id, name, before.map(|v|v.registry_type), before.map(|v|&v.bytes), after.map(|v|v.registry_type), after.map(|v|&v.bytes), time, if gap {"gap-net"} else {"observed"}]).map_err(|e|e.to_string())?;
    }
    tx.execute("DELETE FROM baseline WHERE scope=?1", [scope_id])
        .map_err(|e| e.to_string())?;
    for value in current {
        tx.execute(
            "INSERT INTO baseline(scope,name,registry_type,raw) VALUES(?1,?2,?3,?4)",
            params![scope_id, value.name, value.registry_type, value.bytes],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

// A recovery outcome describes how the current snapshot compares to the intent.
// It does not establish the actor, the exact write time, or intervening changes.
pub(crate) fn recover(
    directory: &Path,
    current: &[Variable],
    readable: &[Scope],
) -> Result<(), String> {
    let pending = pending(directory)?;
    let mut db = connection(directory)?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let mut unresolved = false;
    for operation in pending {
        if !readable.contains(&operation.scope) {
            unresolved = true;
            continue;
        }
        let after = current
            .iter()
            .find(|v| v.scope == operation.scope && crate::same_name(&v.name, &operation.name));
        let outcome = if after == operation.desired_after.as_ref() {
            "applied"
        } else if after == operation.expected_before.as_ref() {
            "not-applied"
        } else {
            "uncertain"
        };
        let changed = tx.execute("UPDATE operations SET after_type=?2,after_raw=?3,time_ms=?4,source='recovery gap (actor unknown; snapshot classification)',outcome=?5 WHERE id=?1 AND outcome='pending'", params![operation.id,after.map(|v|v.registry_type),after.map(|v|&v.bytes),now(),outcome]).map_err(|e|e.to_string())?;
        if changed == 1 {
            replace_baseline(
                &tx,
                if operation.scope == Scope::User { 0 } else { 1 },
                &operation.name,
                after,
            )?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    if unresolved {
        Err("Pending write cannot be classified while its Windows scope is unreadable".into())
    } else {
        Ok(())
    }
}
fn replace_baseline(
    tx: &rusqlite::Transaction<'_>,
    scope: i32,
    name: &str,
    after: Option<&Variable>,
) -> Result<(), String> {
    let names = {
        let mut query = tx
            .prepare("SELECT name FROM baseline WHERE scope=?1")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([scope], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
    };
    for stored in names
        .into_iter()
        .filter(|stored| crate::same_name(stored, name))
    {
        tx.execute(
            "DELETE FROM baseline WHERE scope=?1 AND name=?2",
            params![scope, stored],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(after) = after {
        tx.execute(
            "INSERT INTO baseline(scope,name,registry_type,raw) VALUES(?1,?2,?3,?4)",
            params![scope, after.name, after.registry_type, after.bytes],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn copy_database(source: &Connection, destination: &mut Connection) -> rusqlite::Result<()> {
    let backup = rusqlite::backup::Backup::new(source, destination)?;
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        match backup.step(32)? {
            rusqlite::backup::StepResult::Done => return Ok(()),
            rusqlite::backup::StepResult::More => { deadline = std::time::Instant::now() + std::time::Duration::from_millis(500); },
            _ if std::time::Instant::now() >= deadline => return Err(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), Some("Backup made no progress within its bounded busy interval; original preserved".into()))),
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}
pub(crate) fn backup_history(directory: &Path, destination: &Path) -> Result<(), String> {
    let source = read_connection(directory)?;
    // Reserving exclusively prevents overwriting personal data or prior backups.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| format!("Backup destination preserved: {e}"))?;
    drop(file);
    let mut backup = Connection::open(destination).map_err(|e| e.to_string())?;
    copy_database(&source, &mut backup).map_err(|e| e.to_string())?;
    let integrity: String = backup
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err("Backup integrity check failed; original preserved".into());
    }
    Ok(())
}

pub(crate) fn startup_preference(
    directory: &Path,
) -> Result<Option<(bool, Option<String>)>, String> {
    use rusqlite::OptionalExtension;
    connection(directory)?
        .query_row(
            "SELECT startup_enabled,startup_command FROM settings WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())
}
pub(crate) fn set_startup_preference(
    directory: &Path,
    enabled: bool,
    command: Option<&str>,
) -> Result<(), String> {
    connection(directory)?.execute("INSERT INTO settings(id,startup_enabled,startup_command) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET startup_enabled=excluded.startup_enabled,startup_command=excluded.startup_command",params![enabled,command]).map(|_|()).map_err(|e|e.to_string())
}
