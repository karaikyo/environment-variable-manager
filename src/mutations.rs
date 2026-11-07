//! A single intent/recheck/write/readback pipeline for persistent-variable mutations.
use crate::{
    same_name, storage, Application, Draft, RegistryWriter, SaveOutcome, Scope, Variable,
    WriteOutcome,
};
impl Application {
    fn mark_write_conflict(&mut self, scope: Scope, name: &str, expected: Option<&Variable>) {
        if let Some(draft) = self
            .drafts
            .iter_mut()
            .find(|d| d.scope == scope && same_name(&d.name, name))
        {
            draft.conflict = true;
        } else if let Some(base) = expected {
            self.drafts.push(Draft {
                scope,
                name: name.into(),
                base: Some(base.clone()),
                registry_type: base.registry_type,
                entries: base.entries().unwrap_or_default(),
                conflict: true,
            });
        }
    }
    pub fn request_delete(&mut self, scope: Scope, name: &str) -> Result<(), String> {
        if self.draft(scope, name).is_some_and(|draft| draft.conflict) {
            return Err(
                "Conflict: reload the current Windows value with Discard before deleting".into(),
            );
        }
        let value = self.select(scope, name).ok_or("Select a saved variable")?;
        if value.read_only() {
            return Err("This variable is read-only".into());
        }
        self.delete_confirmation = Some(value.clone());
        Ok(())
    }
    pub fn delete_confirmation(&self) -> Option<&Variable> {
        self.delete_confirmation.as_ref()
    }
    pub fn cancel_delete(&mut self) {
        self.delete_confirmation = None;
    }
    pub fn confirm_delete(&mut self, registry: &dyn RegistryWriter) -> Result<SaveOutcome, String> {
        let expected = self
            .delete_confirmation
            .clone()
            .ok_or("Confirm deletion inline first")?;
        let result = self.apply_user_change(
            registry,
            expected.scope,
            &expected.name,
            Some(&expected),
            None,
        );
        if result.is_ok() {
            self.delete_confirmation = None;
        }
        result
    }
    pub(crate) fn apply_user_change(
        &mut self,
        registry: &dyn RegistryWriter,
        scope: Scope,
        name: &str,
        expected: Option<&Variable>,
        desired: Option<&Variable>,
    ) -> Result<SaveOutcome, String> {
        if self.draft(scope, name).is_some_and(|draft| draft.conflict) {
            return Err(
                "Conflict: reload the current Windows value with Discard before writing".into(),
            );
        }
        if !crate::valid_name(name)
            || [expected, desired].into_iter().flatten().any(|value| {
                value.read_only() || value.scope != scope || !same_name(&value.name, name)
            })
        {
            return Err("Unsupported variable name, type or size".into());
        }
        if scope == Scope::System {
            crate::elevation::SystemRequest {
                name: name.into(),
                expected: expected.cloned(),
                desired: desired.cloned(),
            }
            .validate()?;
        }
        if let Some(error) = &self.storage_error {
            return Err(error.clone());
        }
        if !self.monitoring_errors.is_empty() || !self.scope_errors.is_empty() {
            return Err(
                "Recording interrupted: restore healthy registry observation before writing".into(),
            );
        }
        let current = registry
            .read_scope(scope)?
            .into_iter()
            .find(|v| same_name(&v.name, name));
        if current.as_ref() != expected {
            self.mark_write_conflict(scope, name, expected);
            return Err("Conflict: Windows changed; Discard reloads the current value and preserves other drafts".into());
        }
        let id =
            storage::intent(&self.data_directory, scope, name, expected, desired).map_err(|e| {
                let message = format!("Recording interrupted: {e}; Windows was not written");
                self.storage_error = Some(message.clone());
                message
            })?;
        let current = registry
            .read_scope(scope)
            .map_err(|error| {
                let message = format!("Pending intent retained after recheck failure: {error}");
                self.storage_error = Some(message.clone());
                message
            })?
            .into_iter()
            .find(|v| same_name(&v.name, name));
        if current.as_ref() != expected {
            self.mark_write_conflict(scope, name, expected);
            storage::abandon(&self.data_directory, id).map_err(|error| {
                self.storage_error = Some(error.clone());
                error
            })?;
            self.observe(registry);
            return Err(
                "Conflict: Windows changed immediately before writing; draft preserved".into(),
            );
        }
        let write = if scope == Scope::System {
            registry.change_system(name, expected, desired)
        } else {
            WriteOutcome::Attempted(match desired {
                Some(value) => registry.write_user(value),
                None => registry.delete_user(name),
            })
        };
        let write = match write {
            WriteOutcome::Attempted(result) => result,
            WriteOutcome::Uncertain => {
                let message =
                    "Uncertain: helper response lost; pending intent retained; Refresh required"
                        .to_owned();
                self.storage_error = Some(message.clone());
                return Err(message);
            }
            refused => {
                if let Err(error) = storage::abandon(&self.data_directory, id) {
                    self.storage_error = Some(error.clone());
                    return Err(error);
                }
                if refused == WriteOutcome::Conflict {
                    self.mark_write_conflict(scope, name, expected);
                }
                self.observe(registry);
                return Err(match refused {
                    WriteOutcome::Cancelled => "Administrator approval cancelled; draft preserved".into(),
                    WriteOutcome::Conflict => "Conflict: Windows changed during approval; Discard reloads the current value".into(),
                    WriteOutcome::Rejected(reason) => format!("System write rejected: {reason}; draft preserved"),
                    _ => unreachable!(),
                });
            }
        };
        let observed = match registry.read_scope(scope) {
            Ok(values) => values.into_iter().find(|v| same_name(&v.name, name)),
            Err(_) => {
                let message = "Uncertain: Windows write attempted, but readback failed; further writes blocked".to_owned();
                self.storage_error = Some(message.clone());
                return Err(message);
            }
        };
        let outcome = if write.is_ok() && observed.as_ref() == desired {
            "saved"
        } else if observed.as_ref() == desired {
            "applied"
        } else if observed.as_ref() == expected {
            "not-applied"
        } else {
            "uncertain"
        };
        let state = match outcome {
            "saved" | "applied" => "Applied: Windows currently matches the requested value",
            "not-applied" => "Not applied: Windows still matches the expected before-state",
            _ => "Uncertain: current Windows value differs from both intended states",
        };
        if let Err(error) = storage::finish(&self.data_directory, id, observed.as_ref(), outcome) {
            let message =
                format!("{state}; result recording failed: {error}; further writes blocked");
            self.storage_error = Some(message.clone());
            return Err(message);
        }
        if outcome != "saved" {
            let message = format!(
                "{state}; write failed or readback differed; further writes blocked until Refresh"
            );
            self.storage_error = Some(message.clone());
            return Err(message);
        }
        self.variables
            .retain(|v| !(v.scope == scope && same_name(&v.name, name)));
        if let Some(baseline) = &mut self.baseline {
            baseline.retain(|v| !(v.scope == scope && same_name(&v.name, name)));
            if let Some(desired) = desired {
                baseline.push(desired.clone());
            }
        }
        if let Some(desired) = desired {
            self.variables.push(desired.clone());
        }
        self.drafts
            .retain(|d| !(d.scope == scope && same_name(&d.name, name)));
        Ok(SaveOutcome::Saved)
    }
}
