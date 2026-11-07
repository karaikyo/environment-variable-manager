//! Application commands and queries for the portable environment manager.
pub mod history_comparison;
mod icons;
pub mod lifecycle;
mod mutations;
pub mod native_lifecycle;
pub mod resume;
pub mod startup;
mod storage;
mod system_libraries;
mod windows_startup;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};

/// Maximum supported raw registry value, including a UTF-16 terminator.
pub const MAX_VALUE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\0', '=']) && name.encode_utf16().count() <= 16383
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub scope: Scope,
    pub name: String,
    pub registry_type: u32,
    pub bytes: Vec<u8>,
}
impl Variable {
    pub fn text(scope: Scope, name: &str, value: &str, expand: bool) -> Self {
        Self {
            scope,
            name: name.into(),
            registry_type: if expand { 2 } else { 1 },
            bytes: value
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    }
    /// Decode supported Windows text without expanding references or trimming content.
    /// Keep original bytes available even when text cannot be represented safely.
    pub fn raw_text(&self) -> Option<String> {
        if !matches!(self.registry_type, 1 | 2) || self.bytes.len() % 2 != 0 {
            return None;
        }
        let mut units: Vec<u16> = self
            .bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        if units.last() == Some(&0) {
            units.pop();
        }
        if units.contains(&0) {
            return None;
        }
        String::from_utf16(&units).ok()
    }
    pub fn entries(&self) -> Option<Vec<String>> {
        self.raw_text()
            .map(|value| value.split(';').map(str::to_owned).collect())
    }
    pub fn read_only(&self) -> bool {
        !valid_name(&self.name) || self.bytes.len() > MAX_VALUE_BYTES || self.raw_text().is_none()
    }
}
/// Registry name identity follows Windows ordinal, case-insensitive comparison.
pub fn same_name(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    unsafe {
        CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            == CSTR_EQUAL
    }
}
pub trait RegistryReader {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String>;
    fn watcher(&self) -> Option<windows_registry::RegistryWatch> {
        None
    }
}
/// A pre-write refusal is authoritative: never infer app attribution from readback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Cancelled,
    Rejected(String),
    Conflict,
    Attempted(Result<(), String>),
    /// Delivery may have reached the writer. Retain the intent for reconciliation.
    Uncertain,
}
pub trait RegistryWriter: RegistryReader {
    fn change_system(
        &self,
        _name: &str,
        _expected: Option<&Variable>,
        _desired: Option<&Variable>,
    ) -> WriteOutcome {
        WriteOutcome::Rejected("System-variable helper unavailable".into())
    }

    fn write_user(&self, value: &Variable) -> Result<(), String>;
    fn delete_user(&self, _name: &str) -> Result<(), String> {
        Err("User-variable deletion is unavailable".into())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    NoChange,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRecord {
    pub id: i64,
    pub scope: Scope,
    pub name: String,
    pub before: Option<Variable>,
    pub after: Option<Variable>,
    pub time_ms: i64,
    pub source: String,
    pub outcome: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOperation {
    pub id: i64,
    pub scope: Scope,
    pub name: String,
    pub expected_before: Option<Variable>,
    pub desired_after: Option<Variable>,
}
#[derive(Clone, Debug)]
pub struct Draft {
    pub scope: Scope,
    pub name: String,
    pub base: Option<Variable>,
    pub registry_type: u32,
    pub entries: Vec<String>,
    pub conflict: bool,
}
impl Draft {
    /// Shared Save gate and inline error; size includes separators and terminator.
    pub fn validation_error(&self) -> Option<&'static str> {
        if !valid_name(&self.name) {
            return Some("Use a nonempty name without NUL or equals (at most 16,383 UTF-16 units)");
        }
        if self.entries.is_empty() || self.entries.iter().any(|v| v.contains(['\0', ';'])) {
            return Some("Each entry must contain no NUL or semicolon; keep at least one entry");
        }
        let units = self
            .entries
            .iter()
            .try_fold(self.entries.len(), |units, entry| {
                units.checked_add(entry.encode_utf16().count())
            });
        if units.is_none_or(|units| units > MAX_VALUE_BYTES / 2) {
            return Some("Value exceeds the supported 16 MiB UTF-16 size, including separators and terminator");
        }
        None
    }
}
pub struct Application {
    variables: Vec<Variable>,
    scope_errors: Vec<(Scope, String)>,
    monitoring_errors: Vec<(Scope, String)>,
    baseline: Option<Vec<Variable>>,
    storage_error: Option<String>,
    drafts: Vec<Draft>,
    delete_confirmation: Option<Variable>,
    data_directory: PathBuf,
}
impl Application {
    pub fn open(reader: &dyn RegistryReader, data_directory: &Path) -> Self {
        let mut app = Self {
            variables: Vec::new(),
            scope_errors: Vec::new(),
            monitoring_errors: Vec::new(),
            baseline: None,
            storage_error: None,
            drafts: Vec::new(),
            delete_confirmation: None,
            data_directory: data_directory.into(),
        };
        for scope in [Scope::User, Scope::System] {
            match reader.read_scope(scope) {
                Ok(mut values) => app.variables.append(&mut values),
                Err(error) => app.scope_errors.push((scope, error)),
            }
        }
        match storage::baseline(
            data_directory,
            app.scope_errors
                .is_empty()
                .then_some(app.variables.as_slice()),
            true,
        ) {
            Ok(baseline) => app.baseline = baseline,
            Err(error) => app.storage_error = Some(error),
        }
        if app.storage_error.is_none() {
            match app.pending_operations() {
                Ok(pending) if !pending.is_empty() => app.storage_error = Some(
                    "Pending write requires reconciliation with Refresh; browsing and Discard remain available"
                        .into(),
                ),
                Err(error) => app.storage_error = Some(error),
                _ => {}
            }
        }
        // Reconcile the successful startup reads without a second, racy enumeration.
        struct StartupSnapshot(Vec<Variable>, Vec<(Scope, String)>);
        impl RegistryReader for StartupSnapshot {
            fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
                if let Some((_, error)) = self.1.iter().find(|(s, _)| *s == scope) {
                    return Err(error.clone());
                }
                Ok(self
                    .0
                    .iter()
                    .filter(|v| v.scope == scope)
                    .cloned()
                    .collect())
            }
        }
        let snapshot = StartupSnapshot(app.variables.clone(), app.scope_errors.clone());
        app.reconcile(&snapshot, true);
        app
    }
    /// Reconcile a registry signal. Signals are hints; persisted state is authoritative.
    pub fn observe(&mut self, reader: &dyn RegistryReader) {
        self.reconcile(reader, false);
    }
    /// A notification/lifecycle gap labels net differences without acknowledging
    /// a pending operation or lifting an existing recording gate.
    pub fn observe_gap(&mut self, reader: &dyn RegistryReader) {
        self.reconcile(reader, true);
    }
    /// Explicit refresh represents an observation gap, not a per-write audit.
    pub fn refresh(&mut self, reader: &dyn RegistryReader) {
        // Recovery is an explicit action. Ordinary watcher signals only update browsing
        // while the recording gate is closed; they cannot acknowledge an intent.
        let mut values = Vec::new();
        let mut readable = Vec::new();
        let mut errors = Vec::new();
        for scope in [Scope::User, Scope::System] {
            match reader.read_scope(scope) {
                Ok(mut current) => {
                    readable.push(scope);
                    values.append(&mut current);
                }
                Err(error) => errors.push((scope, error)),
            }
        }
        if self.storage_error.is_some() {
            let recovery = (|| {
                storage::baseline(
                    &self.data_directory,
                    errors.is_empty().then_some(values.as_slice()),
                    false,
                )?;
                storage::recover(&self.data_directory, &values, &readable)?;
                self.baseline = storage::baseline(&self.data_directory, None, false)?;
                Ok::<(), String>(())
            })();
            self.storage_error = recovery
                .err()
                .map(|error| format!("Recording interrupted: {error}"));
        }
        struct Snapshot(Vec<Variable>, Vec<(Scope, String)>);
        impl RegistryReader for Snapshot {
            fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
                if let Some((_, error)) = self.1.iter().find(|(s, _)| *s == scope) {
                    return Err(error.clone());
                }
                Ok(self
                    .0
                    .iter()
                    .filter(|v| v.scope == scope)
                    .cloned()
                    .collect())
            }
        }
        self.reconcile(&Snapshot(values, errors), true);
    }
    fn reconcile(&mut self, reader: &dyn RegistryReader, gap: bool) {
        for scope in [Scope::User, Scope::System] {
            let interrupted = self.scope_error(scope).is_some();
            let values = match reader.read_scope(scope) {
                Ok(values) => values,
                Err(error) => {
                    self.scope_errors.retain(|(s, _)| *s != scope);
                    self.scope_errors.push((scope, error));
                    continue;
                }
            };
            self.scope_errors.retain(|(s, _)| *s != scope);
            for draft in &mut self.drafts {
                if draft.scope == scope {
                    let current = values.iter().find(|v| same_name(&v.name, &draft.name));
                    if current != draft.base.as_ref() {
                        draft.conflict = true;
                    }
                }
            }
            self.variables.retain(|v| v.scope != scope);
            self.variables.extend(values.clone());
            if self.storage_error.is_some() || !self.monitoring_errors.is_empty() {
                continue;
            }
            if let Some(baseline) = &mut self.baseline {
                let previous: Vec<_> = baseline
                    .iter()
                    .filter(|v| v.scope == scope)
                    .cloned()
                    .collect();
                match storage::observation(
                    &self.data_directory,
                    scope,
                    &previous,
                    &values,
                    gap || interrupted || self.monitoring_errors.iter().any(|(s, _)| *s == scope),
                ) {
                    Ok(()) => {
                        baseline.retain(|v| v.scope != scope);
                        baseline.extend(values);
                    }
                    Err(error) => {
                        self.storage_error = Some(format!("Recording interrupted: {error}"));
                    }
                }
            }
        }
        if self.baseline.is_none() && self.scope_errors.is_empty() && self.storage_error.is_none() {
            match storage::baseline(&self.data_directory, Some(&self.variables), false) {
                Ok(baseline) => self.baseline = baseline,
                Err(error) => self.storage_error = Some(error),
            }
        }
    }
    pub fn monitoring_errors(&self) -> &[(Scope, String)] {
        &self.monitoring_errors
    }
    pub fn set_monitoring_errors(&mut self, errors: Vec<(Scope, String)>) {
        self.monitoring_errors = errors;
    }
    pub fn storage_error(&self) -> Option<&str> {
        self.storage_error.as_deref()
    }
    pub fn baseline(&self) -> Option<&[Variable]> {
        self.baseline.as_deref()
    }
    pub fn scope_error(&self, scope: Scope) -> Option<&str> {
        self.scope_errors
            .iter()
            .find(|(s, _)| *s == scope)
            .map(|(_, error)| error.as_str())
    }
    pub fn browse(&self, query: &str, scope: Option<Scope>) -> Vec<&Variable> {
        let query = query.to_lowercase();
        self.variables
            .iter()
            .filter(|v| scope.is_none_or(|scope| scope == v.scope))
            .filter(|v| {
                v.name.to_lowercase().contains(&query)
                    || v.raw_text()
                        .is_some_and(|s| s.to_lowercase().contains(&query))
            })
            .collect()
    }
    pub fn select(&self, scope: Scope, name: &str) -> Option<&Variable> {
        self.variables
            .iter()
            .find(|v| v.scope == scope && same_name(&v.name, name))
    }
    /// Session edits include creations, invalid edits and edits on unselected variables.
    pub fn has_unsaved_edits(&self) -> bool {
        self.drafts.iter().any(|d| {
            d.entries.is_empty()
                || d.base.as_ref().is_none_or(|base| {
                    d.registry_type != base.registry_type
                        || Some(d.entries.join(";")) != base.raw_text()
                })
        })
    }
    /// Explicit exit only: saved values, history and pending recovery are untouched.
    pub fn discard_session_drafts(&mut self) {
        self.drafts.clear();
        self.delete_confirmation = None;
    }
    pub fn drafts(&self) -> impl Iterator<Item = &Draft> {
        self.drafts.iter()
    }
    pub fn draft(&self, scope: Scope, name: &str) -> Option<&Draft> {
        self.drafts
            .iter()
            .find(|d| d.scope == scope && same_name(&d.name, name))
    }
    pub fn edit_entries(
        &mut self,
        scope: Scope,
        name: &str,
        entries: Vec<String>,
    ) -> Result<(), String> {
        if let Some(draft) = self
            .drafts
            .iter_mut()
            .find(|d| d.scope == scope && same_name(&d.name, name))
        {
            draft.entries = entries;
            return Ok(());
        }
        let base = self
            .select(scope, name)
            .ok_or("Variable no longer exists")?
            .clone();
        if base.read_only() {
            return Err("This variable is read-only".into());
        }
        if let Some(draft) = self
            .drafts
            .iter_mut()
            .find(|d| d.scope == scope && same_name(&d.name, name))
        {
            draft.entries = entries;
        } else {
            self.drafts.push(Draft {
                scope,
                name: base.name.clone(),
                registry_type: base.registry_type,
                base: Some(base),
                entries,
                conflict: false,
            });
        }
        Ok(())
    }
    /// Start an unsaved creation; absence is its expected before-state.
    pub fn create_user(&mut self, name: &str) -> Result<(), String> {
        self.create_variable(Scope::User, name)
    }
    pub fn create_system(&mut self, name: &str) -> Result<(), String> {
        self.create_variable(Scope::System, name)
    }
    pub fn create_variable(&mut self, scope: Scope, name: &str) -> Result<(), String> {
        if !valid_name(name) {
            return Err(
                "Use a nonempty name without NUL or equals (at most 16,383 UTF-16 units)".into(),
            );
        }
        if self.select(scope, name).is_some() || self.draft(scope, name).is_some() {
            return Err("A variable or draft with this name already exists".into());
        }
        self.drafts.push(Draft {
            scope,
            name: name.into(),
            base: None,
            registry_type: 1,
            entries: vec![String::new()],
            conflict: false,
        });
        Ok(())
    }
    pub fn set_new_expand(&mut self, name: &str, expand: bool) -> Result<(), String> {
        self.set_new_expand_in(Scope::User, name, expand)
    }
    pub fn set_new_expand_in(
        &mut self,
        scope: Scope,
        name: &str,
        expand: bool,
    ) -> Result<(), String> {
        let draft = self
            .drafts
            .iter_mut()
            .find(|d| d.scope == scope && same_name(&d.name, name) && d.base.is_none())
            .ok_or("Select an unsaved new variable")?;
        draft.registry_type = if expand { 2 } else { 1 };
        Ok(())
    }
    pub fn can_save(&self, scope: Scope, name: &str) -> bool {
        self.storage_error.is_none()
            && self.monitoring_errors.is_empty()
            && self.scope_errors.is_empty()
            && self.draft(scope, name).is_some_and(|d| {
                !d.conflict
                    && d.validation_error().is_none()
                    && Some(d.entries.join(";")) != d.base.as_ref().and_then(Variable::raw_text)
            })
    }
    pub fn discard(
        &mut self,
        reader: &dyn RegistryReader,
        scope: Scope,
        name: &str,
    ) -> Result<(), String> {
        if self
            .draft(scope, name)
            .is_some_and(|draft| draft.base.is_none())
        {
            self.drafts
                .retain(|d| !(d.scope == scope && same_name(&d.name, name)));
            return Ok(());
        }
        let current = reader
            .read_scope(scope)?
            .into_iter()
            .find(|v| same_name(&v.name, name));
        self.variables
            .retain(|v| !(v.scope == scope && same_name(&v.name, name)));
        if let Some(current) = current {
            self.variables.push(current);
        }
        self.drafts
            .retain(|d| !(d.scope == scope && same_name(&d.name, name)));
        Ok(())
    }
    pub fn history(
        &self,
        selected: Option<(Scope, &str)>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<HistoryRecord>, String> {
        storage::records(&self.data_directory, false, selected, offset, limit)
    }
    /// Explicitly clear completed history for the active filter only.
    pub fn clear_history(&self, selected: Option<(Scope, &str)>) -> Result<usize, String> {
        storage::clear_records(&self.data_directory, selected)
    }
    /// Copy the live original database through SQLite, verify it, and never replace
    /// an existing destination. This copies data; it never restores Windows values.
    pub fn backup_history(&self, destination: &Path) -> Result<(), String> {
        storage::backup_history(&self.data_directory, destination)
    }
    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }
    pub fn pending_operations(&self) -> Result<Vec<PendingOperation>, String> {
        storage::pending(&self.data_directory)
    }
    pub fn save(
        &mut self,
        registry: &dyn RegistryWriter,
        scope: Scope,
        name: &str,
    ) -> Result<SaveOutcome, String> {
        let Some(draft) = self.draft(scope, name).cloned() else {
            return Ok(SaveOutcome::NoChange);
        };
        if draft.conflict {
            return Err(
                "Conflict: reload the current Windows value with Discard before saving".into(),
            );
        }
        if draft.base.as_ref().is_some_and(Variable::read_only) {
            return Err("This variable is read-only".into());
        }
        if let Some(error) = draft.validation_error() {
            return Err(error.into());
        }
        let desired = Variable::text(
            scope,
            &draft.name,
            &draft.entries.join(";"),
            draft.registry_type == 2,
        );
        // Text-equivalent drafts do not normalize unusual but supported registry bytes.
        if desired.raw_text() == draft.base.as_ref().and_then(Variable::raw_text) {
            self.drafts
                .retain(|d| !(d.scope == scope && same_name(&d.name, name)));
            return Ok(SaveOutcome::NoChange);
        }
        self.apply_user_change(
            registry,
            scope,
            &draft.name,
            draft.base.as_ref(),
            Some(&desired),
        )
    }
}

pub mod desktop;
pub mod elevation;
pub mod history_window;
pub mod windows_registry;
