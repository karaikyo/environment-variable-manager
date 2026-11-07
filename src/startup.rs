pub use crate::windows_startup::WindowsStartup;
// Sign-in preference. Only an explicit setting change may re-enable startup.
use crate::{storage, Application};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Default)]
pub struct RegistrationState {
    pub command: Option<String>,
    pub blocked: bool,
}
/// System boundary: inspect and conditionally update only the application's value.
pub trait StartupRegistration {
    fn state(&self) -> Result<RegistrationState, String>;
    fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String>;
}
pub struct Startup {
    directory: PathBuf,
    enabled: bool,
    preferred_enabled: bool,
    registered: Option<String>,
    command: String,
}
impl Startup {
    pub fn initialize(
        app: &Application,
        registration: &dyn StartupRegistration,
        executable: &Path,
    ) -> Result<Self, String> {
        if app.storage_error().is_some()
            || app.baseline().is_none()
            || !app.monitoring_errors().is_empty()
            || [crate::Scope::User, crate::Scope::System]
                .iter()
                .any(|s| app.scope_error(*s).is_some())
        {
            return Err("Startup unchanged: recording initialization is not healthy".into());
        }
        let command = format!("\"{}\" --tray", executable.display());
        let saved = storage::startup_preference(app.data_directory())?;
        let state = registration.state()?;
        let mut startup = Self {
            directory: app.data_directory().into(),
            enabled: false,
            preferred_enabled: saved.as_ref().is_some_and(|v| v.0),
            registered: saved.as_ref().and_then(|v| v.1.clone()),
            command,
        };
        match saved {
            None => {
                // Commit the one-time decision before touching Run. Failure is never retried silently.
                startup.persist(false)?;
                if !state.blocked && state.command.is_none() {
                    startup.set_enabled(true, registration)?;
                }
            }
            Some((enabled, _)) => {
                startup.enabled = enabled
                    && !state.blocked
                    && state.command.is_some()
                    && state.command == startup.registered;
                if enabled && (state.command.is_none() || state.command != startup.registered) {
                    startup.persist(false)?;
                } else if startup.enabled && startup.registered.as_ref() != Some(&startup.command) {
                    // Same-account manual folder move: replace only the exact
                    // previously owned, still enabled and unblocked command.
                    startup.set_enabled(true, registration)?;
                }
            }
        }
        Ok(startup)
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    /// The user's saved choice is distinct from a current Windows policy block.
    pub fn preferred_enabled(&self) -> bool {
        self.preferred_enabled
    }
    /// Recheck Windows without registering or re-enabling anything.
    pub fn refresh_status(&mut self, registration: &dyn StartupRegistration) -> Result<(), String> {
        let state = registration.state()?;
        let owned = state.command.is_some() && state.command == self.registered;
        if self.preferred_enabled && !owned {
            self.persist(false)?;
        }
        self.enabled = self.preferred_enabled && owned && !state.blocked;
        Ok(())
    }
    fn persist(&mut self, enabled: bool) -> Result<(), String> {
        storage::set_startup_preference(&self.directory, enabled, self.registered.as_deref())?;
        self.preferred_enabled = enabled;
        self.enabled = enabled;
        Ok(())
    }
    pub fn set_enabled(
        &mut self,
        enabled: bool,
        registration: &dyn StartupRegistration,
    ) -> Result<(), String> {
        let state = registration.state()?;
        if enabled && state.blocked {
            return Err("Sign-in startup is disabled by Windows settings or policy".into());
        }
        if state.command.is_some() && state.command != self.registered {
            return Err(
                "Startup entry changed outside the application; existing entry preserved".into(),
            );
        }
        if enabled {
            // Persist the decision and ownership before Run. If interrupted before
            // registration, its absence on the next launch is an external disable,
            // never a reason to retry registration automatically.
            self.registered = Some(self.command.clone());
            self.persist(true)?;
            if let Err(error) = registration.replace(state.command.as_deref(), Some(&self.command))
            {
                self.persist(false)?;
                return Err(error);
            }
            Ok(())
        } else {
            if state.command.is_some() {
                registration.replace(state.command.as_deref(), None)?;
            }
            self.persist(false)
        }
    }
}
