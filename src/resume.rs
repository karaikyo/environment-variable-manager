//! Power-event coordination. All native registry registrations remain worker-owned.
use crate::{windows_registry::RegistryWatch, Scope};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMECRITICAL, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
};

#[derive(Clone, Default)]
pub struct Monitoring {
    pub changed: bool,
    pub gap: bool,
    pub errors: Vec<(Scope, String)>,
    suspended: bool,
    rearm: bool,
}
impl Monitoring {
    /// Accept native power notifications, including a resume without a prior suspend.
    /// Overlapping automatic/user resume notifications are harmless snapshot hints.
    pub fn power_event(&mut self, event: u32) -> bool {
        match event {
            PBT_APMSUSPEND => self.suspended = true,
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => {
                self.suspended = false;
                self.rearm = true;
            }
            _ => return false,
        }
        self.changed = true;
        self.gap = true;
        self.errors = [Scope::User, Scope::System]
            .into_iter()
            .map(|s| {
                (
                    s,
                    "Monitoring interrupted by power transition; waiting to rearm observation"
                        .into(),
                )
            })
            .collect();
        true
    }
    /// Run on the data worker before enumerating or accepting a write. Reconnect
    /// notifications before reading the snapshot, retaining concurrent signals.
    pub fn poll(&mut self, watch: Option<&mut RegistryWatch>) {
        if self.suspended {
            return;
        }
        let Some(watch) = watch else {
            return;
        };
        let status = if self.rearm {
            self.rearm = false;
            watch.resume()
        } else {
            watch.poll(std::time::Duration::ZERO)
        };
        self.changed |= status.changed || status.errors != self.errors;
        self.gap |= status.gap;
        self.errors = status.errors;
    }
}
