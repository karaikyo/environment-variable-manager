//! Session lifecycle decisions. Persisted operations stay owned by the data worker.
use crate::Application;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitDecision {
    WaitForOperation,
    ConfirmDiscard,
    Exit,
}
pub struct Lifecycle {
    visible: bool,
    exit_requested: bool,
    confirming: bool,
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            visible: true,
            exit_requested: false,
            confirming: false,
        }
    }
}
impl Lifecycle {
    pub fn visible(&self) -> bool {
        self.visible
    }
    /// A failed tray insertion must leave the main window reachable.
    pub fn close_to_tray(&mut self, tray_available: bool) -> bool {
        self.cancel_exit();
        self.visible = !tray_available;
        tray_available
    }
    pub fn reopen(&mut self) {
        self.visible = true;
    }
    pub fn request_exit(&mut self, app: Option<&Application>) -> ExitDecision {
        self.exit_requested = true;
        let Some(app) = app else {
            return ExitDecision::WaitForOperation;
        };
        if app.has_unsaved_edits() {
            self.visible = true;
            self.confirming = true;
            ExitDecision::ConfirmDiscard
        } else {
            ExitDecision::Exit
        }
    }
    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }
    pub fn confirming(&self) -> bool {
        self.confirming
    }
    pub fn cancel_exit(&mut self) {
        self.exit_requested = false;
        self.confirming = false;
    }
    pub fn confirm_exit(&mut self, app: &mut Application) {
        app.discard_session_drafts();
        self.confirming = false;
    }
}
