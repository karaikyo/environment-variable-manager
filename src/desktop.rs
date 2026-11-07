//! Native list-and-detail window. All HWND state stays on the message-loop thread.
use crate::{
    lifecycle::{ExitDecision, Lifecycle},
    native_lifecycle::{self, Tray},
    startup::{Startup, StartupRegistration, WindowsStartup},
};
use crate::{Application, RegistryWriter, Scope};
use std::{
    cell::RefCell,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::*,
        HiDpi::*,
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::*,
    },
};

const SEARCH: usize = 101;
const FILTER: usize = 102;
const VARIABLES: usize = 103;
const ENTRIES: usize = 104;
const ENTRY: usize = 105;
const ADD: usize = 106;
const REMOVE: usize = 107;
const UP: usize = 108;
const DOWN: usize = 109;
const SAVE: usize = 110;
const DISCARD: usize = 111;
const HISTORY: usize = 112;
const REFRESH: usize = 113;
const NEW_NAME: usize = 130;
const NEW: usize = 131;
const EXPAND: usize = 132;
const DELETE: usize = 133;
const CONFIRM_DELETE: usize = 134;
const CANCEL_DELETE: usize = 135;
const STARTUP: usize = 152;
const CONFIRM_EXIT: usize = 153;
const CANCEL_EXIT: usize = 154;
const LOAD_TIMER: usize = 1;
#[derive(Clone, Copy)]
enum RefreshKind {
    Live,
    Gap,
    Recover,
}
enum Work {
    Save(Application, Scope, String),
    Startup(Application, bool),
    StartupStatus(Application),
    Refresh(Application, RefreshKind),
    Delete(Application),
    Discard(Application, Scope, String),
}
struct Response {
    startup: Option<Result<bool, String>>,
    app: Application,
    message: Option<String>,
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn scope_name(scope: Scope) -> &'static str {
    if scope == Scope::User {
        "User"
    } else {
        "System"
    }
}

struct WindowState {
    lifecycle: Lifecycle,
    tray: Option<Tray>,
    recording_interrupted: bool,
    worker_stopped: bool,
    startup: HWND,
    exit: HWND,
    confirm_exit: HWND,
    cancel_exit: HWND,
    monitor: std::sync::Arc<std::sync::Mutex<crate::resume::Monitoring>>,
    receiver: Receiver<Response>,
    sender: SyncSender<Work>,
    app: Option<Application>,
    visible: Vec<(Scope, String)>,
    entry_values: Vec<String>,
    filling: bool,
    dpi: u32,
    font: HFONT,
    search_label: HWND,
    search: HWND,
    filter_label: HWND,
    filter: HWND,
    variables: HWND,
    detail: HWND,
    new_label: HWND,
    new_name: HWND,
    new_button: HWND,
    expand: HWND,
    delete: HWND,
    confirm_delete: HWND,
    cancel_delete: HWND,
    entries: HWND,
    entry_label: HWND,
    entry: HWND,
    status: HWND,
    buttons: Vec<HWND>,
}
impl WindowState {
    fn new(
        receiver: Receiver<Response>,
        sender: SyncSender<Work>,
        monitor: std::sync::Arc<std::sync::Mutex<crate::resume::Monitoring>>,
    ) -> Self {
        Self {
            lifecycle: Lifecycle::default(),
            tray: None,
            recording_interrupted: true,
            worker_stopped: false,
            startup: null_mut(),
            exit: null_mut(),
            confirm_exit: null_mut(),
            cancel_exit: null_mut(),
            monitor,
            receiver,
            sender,
            app: None,
            visible: Vec::new(),
            entry_values: Vec::new(),
            filling: false,
            dpi: 96,
            font: null_mut(),
            search_label: null_mut(),
            search: null_mut(),
            filter_label: null_mut(),
            filter: null_mut(),
            variables: null_mut(),
            detail: null_mut(),
            new_label: null_mut(),
            new_name: null_mut(),
            new_button: null_mut(),
            expand: null_mut(),
            delete: null_mut(),
            confirm_delete: null_mut(),
            cancel_delete: null_mut(),
            entries: null_mut(),
            entry_label: null_mut(),
            entry: null_mut(),
            status: null_mut(),
            buttons: Vec::new(),
        }
    }
    unsafe fn create(&mut self, parent: HWND) {
        self.dpi = GetDpiForWindow(parent).max(96);
        self.startup = child(
            parent,
            "BUTTON",
            "Start at sign-in",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            STARTUP,
        );
        self.exit = child(
            parent,
            "BUTTON",
            "E&xit",
            WS_TABSTOP,
            0,
            native_lifecycle::EXIT,
        );
        self.confirm_exit = child(
            parent,
            "BUTTON",
            "Exit and discard unsaved edits",
            WS_TABSTOP,
            0,
            CONFIRM_EXIT,
        );
        self.cancel_exit = child(parent, "BUTTON", "Cancel exit", WS_TABSTOP, 0, CANCEL_EXIT);
        ShowWindow(self.confirm_exit, SW_HIDE);
        ShowWindow(self.cancel_exit, SW_HIDE);
        EnableWindow(self.startup, 0);
        let mut tray = Tray::new(parent);
        tray.update(true);
        self.tray = Some(tray);
        self.search_label = child(parent, "STATIC", "&Search names and values", 0, 0, 0);
        self.search = child(
            parent,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            SEARCH,
        );
        self.filter_label = child(parent, "STATIC", "S&cope", 0, 0, 0);
        self.filter = child(
            parent,
            "COMBOBOX",
            "",
            WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
            0,
            FILTER,
        );
        for text in ["All scopes", "User variables", "System variables"] {
            SendMessageW(self.filter, CB_ADDSTRING, 0, wide(text).as_ptr() as isize);
        }
        SendMessageW(self.filter, CB_SETCURSEL, 0, 0);
        self.variables = child(
            parent,
            "SysListView32",
            "Variables by scope and name",
            WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
            WS_EX_CLIENTEDGE,
            VARIABLES,
        );
        configure_list(self.variables, &["Scope", "Variable name"]);
        self.detail = child(parent, "STATIC", "Select a variable", 0, 0, 0);
        self.new_label = child(parent, "STATIC", "New &user variable name", 0, 0, 0);
        self.new_name = child(
            parent,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            NEW_NAME,
        );
        SendMessageW(self.new_name, EM_SETLIMITTEXT, 16383, 0);
        self.new_button = child(
            parent,
            "BUTTON",
            "&New",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            0,
            NEW,
        );
        self.expand = child(
            parent,
            "BUTTON",
            "Expand %NAME% references",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            0,
            EXPAND,
        );
        self.delete = child(
            parent,
            "BUTTON",
            "&Delete variable",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            0,
            DELETE,
        );
        self.confirm_delete = child(
            parent,
            "BUTTON",
            "Confirm &delete",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            0,
            CONFIRM_DELETE,
        );
        self.cancel_delete = child(
            parent,
            "BUTTON",
            "&Cancel deletion",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            0,
            CANCEL_DELETE,
        );
        ShowWindow(self.confirm_delete, SW_HIDE);
        ShowWindow(self.cancel_delete, SW_HIDE);
        self.entries = child(
            parent,
            "SysListView32",
            "Value entries in stored order",
            WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
            WS_EX_CLIENTEDGE,
            ENTRIES,
        );
        configure_list(
            self.entries,
            &["Entry", "Value (line breaks shown as \\r and \\n)"],
        );
        self.entry_label = child(parent, "STATIC", "Selected &entry — exact text", 0, 0, 0);
        self.entry = child(
            parent,
            "EDIT",
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE as u32
                | ES_WANTRETURN as u32
                | ES_AUTOVSCROLL as u32
                | ES_AUTOHSCROLL as u32
                | ES_READONLY as u32,
            WS_EX_CLIENTEDGE,
            ENTRY,
        );
        SendMessageW(
            self.entry,
            EM_SETLIMITTEXT,
            crate::MAX_VALUE_BYTES / 2 - 1,
            0,
        );
        self.status = child(
            parent,
            "STATIC",
            "Reading saved variables and local baseline…",
            0,
            0,
            0,
        );
        self.set_font();
        for (id, text) in [
            (ADD, "&Add entry"),
            (REMOVE, "&Remove"),
            (UP, "Move &up"),
            (DOWN, "Move &down"),
            (SAVE, "&Save"),
            (DISCARD, "D&iscard"),
            (HISTORY, "&History"),
            (REFRESH, "Re&fresh"),
        ] {
            let button = child(parent, "BUTTON", text, WS_TABSTOP, 0, id);
            SendMessageW(button, WM_SETFONT, self.font as usize, 1);
            EnableWindow(button, 0);
            self.buttons.push(button);
        }
        SetTimer(parent, LOAD_TIMER, 100, None);
        SetFocus(self.search);
    }
    unsafe fn set_font(&mut self) {
        let previous = self.font;
        self.font = CreateFontW(
            -(9 * self.dpi as i32 / 72),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            DEFAULT_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide("Segoe UI").as_ptr(),
        );
        for hwnd in [
            self.startup,
            self.exit,
            self.confirm_exit,
            self.cancel_exit,
            self.search_label,
            self.search,
            self.filter_label,
            self.filter,
            self.variables,
            self.detail,
            self.new_label,
            self.new_name,
            self.new_button,
            self.expand,
            self.delete,
            self.confirm_delete,
            self.cancel_delete,
            self.entries,
            self.entry_label,
            self.entry,
            self.status,
        ] {
            SendMessageW(hwnd, WM_SETFONT, self.font as usize, 1);
        }
        for hwnd in &self.buttons {
            SendMessageW(*hwnd, WM_SETFONT, self.font as usize, 1);
        }
        if !previous.is_null() {
            DeleteObject(previous);
        }
    }
    unsafe fn layout(&self, parent: HWND) {
        let mut rect = RECT::default();
        GetClientRect(parent, &mut rect);
        let scale = |n: i32| n * self.dpi as i32 / 96;
        let margin = scale(12);
        let gap = scale(12);
        let label = scale(22);
        let field = scale(28);
        let width = rect.right;
        let height = rect.bottom;
        let left = (width * 37 / 100).max(scale(240));
        let right_x = left + gap;
        let right_w = (width - right_x - margin).max(1);
        let top = margin + label + field + gap;
        let bottom = (height - scale(156)).max(top + scale(100));
        MoveWindow(self.search_label, margin, margin, left - margin, label, 1);
        MoveWindow(self.search, margin, margin + label, left - margin, field, 1);
        MoveWindow(self.filter_label, right_x, margin, scale(200), label, 1);
        MoveWindow(
            self.filter,
            right_x,
            margin + label,
            scale(220).min(right_w),
            scale(180),
            1,
        );
        MoveWindow(self.variables, margin, top, left - margin, bottom - top, 1);
        MoveWindow(self.detail, right_x, top, right_w, scale(66), 1);
        MoveWindow(
            self.new_label,
            right_x + scale(232),
            margin,
            right_w - scale(232),
            label,
            1,
        );
        MoveWindow(
            self.new_name,
            right_x + scale(232),
            margin + label,
            (right_w - scale(310)).max(40),
            field,
            1,
        );
        MoveWindow(
            self.new_button,
            width - margin - scale(70),
            margin + label,
            scale(70),
            field,
            1,
        );
        MoveWindow(self.expand, right_x, top + scale(44), right_w, scale(26), 1);
        let entry_top = top + scale(72);
        let entries_height = ((bottom - entry_top) * 55 / 100).max(scale(60));
        MoveWindow(self.entries, right_x, entry_top, right_w, entries_height, 1);
        let editor_top = entry_top + entries_height + scale(8);
        MoveWindow(self.entry_label, right_x, editor_top, right_w, label, 1);
        MoveWindow(
            self.entry,
            right_x,
            editor_top + label,
            right_w,
            (bottom - editor_top - label).max(1),
            1,
        );
        MoveWindow(
            self.status,
            margin,
            bottom + scale(46),
            (width - 2 * margin - scale(250)).max(1),
            scale(64),
            1,
        );
        let button_width = ((width - 2 * margin) / 9).min(scale(130));
        for (i, button) in self.buttons.iter().enumerate() {
            MoveWindow(
                *button,
                margin + i as i32 * button_width,
                bottom + scale(8),
                button_width - scale(4),
                scale(30),
                1,
            );
        }
        MoveWindow(
            self.delete,
            margin + 8 * button_width,
            bottom + scale(8),
            button_width - scale(4),
            scale(30),
            1,
        );
        MoveWindow(
            self.confirm_delete,
            width - margin - scale(245),
            bottom + scale(46),
            scale(120),
            scale(30),
            1,
        );
        MoveWindow(
            self.cancel_delete,
            width - margin - scale(120),
            bottom + scale(46),
            scale(120),
            scale(30),
            1,
        );
        MoveWindow(
            self.startup,
            margin,
            height - scale(36),
            scale(180),
            scale(28),
            1,
        );
        MoveWindow(
            self.exit,
            width - margin - scale(80),
            height - scale(36),
            scale(80),
            scale(28),
            1,
        );
        MoveWindow(
            self.confirm_exit,
            margin + scale(185),
            height - scale(36),
            scale(260),
            scale(28),
            1,
        );
        MoveWindow(
            self.cancel_exit,
            margin + scale(450),
            height - scale(36),
            scale(110),
            scale(28),
            1,
        );
        SendMessageW(self.variables, LVM_SETCOLUMNWIDTH, 0, scale(65) as isize);
        SendMessageW(
            self.variables,
            LVM_SETCOLUMNWIDTH,
            1,
            (left - margin - scale(90)).max(80) as isize,
        );
        SendMessageW(self.entries, LVM_SETCOLUMNWIDTH, 0, scale(55) as isize);
        SendMessageW(
            self.entries,
            LVM_SETCOLUMNWIDTH,
            1,
            (right_w - scale(80)).max(80) as isize,
        );
    }
    unsafe fn loaded(&mut self, parent: HWND) {
        crate::history_window::workers_settled();
        match self.receiver.try_recv() {
            Ok(response) => {
                let mut response = response;
                self.app = Some(response.app);
                if let Some(startup) = response.startup {
                    SendMessageW(
                        self.startup,
                        BM_SETCHECK,
                        if startup.as_ref().is_ok_and(|v| *v) {
                            BST_CHECKED
                        } else {
                            BST_UNCHECKED
                        } as usize,
                        0,
                    );
                    if let Err(error) = startup {
                        response.message = Some(format!(
                            "{}\r\n{error}",
                            response.message.take().unwrap_or_default()
                        ));
                    }
                }
                EnableWindow(self.startup, 1);
                self.populate();
                EnableWindow(self.entry, 1);
                for hwnd in [self.search, self.filter, self.variables] {
                    EnableWindow(hwnd, 1);
                }
                self.edit_gate();
                if let Some(message) = response.message {
                    SetWindowTextW(
                        self.status,
                        wide(&format!("{message}\r\n{}", window_text(self.status))).as_ptr(),
                    );
                }
                SetWindowTextW(parent, wide("Environment Manager").as_ptr());
            }
            Err(TryRecvError::Disconnected) => {
                self.worker_stopped = true;
                self.recording_interrupted = true;
                self.reopen(parent);
                KillTimer(parent, LOAD_TIMER);
                SetWindowTextW(self.status, wide("Loading failed. Close and reopen the application; existing data is preserved.").as_ptr());
            }
            Err(TryRecvError::Empty) => {}
        }
        self.update_tray(parent, false);
        if self.lifecycle.exit_requested() && !self.lifecycle.confirming() {
            self.request_exit(parent);
            return;
        }
        let signal = {
            let mut monitor = self.monitor.lock().unwrap();
            let pending = self
                .app
                .as_ref()
                .is_some_and(|app| monitor.changed || monitor.errors != app.monitoring_errors());
            if pending {
                let status = monitor.clone();
                monitor.changed = false;
                monitor.gap = false;
                Some(status)
            } else {
                None
            }
        };
        if let Some(signal) = signal {
            self.refresh(if signal.gap {
                RefreshKind::Gap
            } else {
                RefreshKind::Live
            });
        }
        if self.lifecycle.confirming() {
            self.exit_prompt();
        }
    }
    unsafe fn populate(&mut self) {
        let Some(app) = &self.app else {
            return;
        };
        let selected = selected_index(self.variables)
            .and_then(|i| self.visible.get(i))
            .cloned();
        let filter = match SendMessageW(self.filter, CB_GETCURSEL, 0, 0) {
            1 => Some(Scope::User),
            2 => Some(Scope::System),
            _ => None,
        };
        SetWindowTextW(
            self.new_label,
            wide(if filter == Some(Scope::System) {
                "New &system variable name"
            } else {
                "New &user variable name"
            })
            .as_ptr(),
        );
        self.visible = app
            .browse(&window_text(self.search), filter)
            .into_iter()
            .map(|v| (v.scope, v.name.clone()))
            .collect();
        let query = window_text(self.search).to_lowercase();
        self.visible.extend(
            app.drafts()
                .filter(|d| app.select(d.scope, &d.name).is_none())
                .filter(|d| filter.is_none_or(|scope| scope == d.scope))
                .filter(|d| {
                    d.name.to_lowercase().contains(&query)
                        || d.entries.join(";").to_lowercase().contains(&query)
                })
                .map(|d| (d.scope, d.name.clone())),
        );
        self.visible.sort_by_key(|(scope, name)| {
            (
                if *scope == Scope::User { 0 } else { 1 },
                name.to_lowercase(),
            )
        });
        self.filling = true;
        SendMessageW(self.variables, LVM_DELETEALLITEMS, 0, 0);
        for (i, (scope, name)) in self.visible.iter().enumerate() {
            insert_row(
                self.variables,
                i,
                &[
                    scope_name(*scope).into(),
                    if name.is_empty() {
                        "(unnamed value)".into()
                    } else {
                        name.clone()
                    },
                ],
            );
        }
        let row = selected
            .and_then(|wanted| {
                self.visible
                    .iter()
                    .position(|v| v.0 == wanted.0 && crate::same_name(&v.1, &wanted.1))
            })
            .unwrap_or(0);
        if !self.visible.is_empty() {
            select_row(self.variables, row);
        }
        self.filling = false;
        let mut lines = Vec::new();
        for scope in [Scope::User, Scope::System] {
            lines.push(match app.scope_error(scope) {
                Some(error) => format!("{}: {error}", scope_name(scope)),
                None => format!(
                    "{}: {} saved values",
                    scope_name(scope),
                    app.browse("", Some(scope)).len()
                ),
            });
        }
        let mut lines = vec![lines.join("; ")];
        if !app.monitoring_errors().is_empty() {
            lines.push(
                app.monitoring_errors()
                    .iter()
                    .map(|(scope, error)| format!("{} monitoring: {error}", scope_name(*scope)))
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
        lines.push(match app.storage_error() {
            Some(error) => error.to_owned(),
            None => match app.baseline() {
                Some(values) => format!(
                    "Local baseline: {} values. Browsing does not change Windows values.",
                    values.len()
                ),
                None => "Baseline pending: both scopes must be readable.".into(),
            },
        });
        SetWindowTextW(self.status, wide(&lines.join("\r\n")).as_ptr());
        self.show_variable();
    }
    unsafe fn show_variable(&mut self) {
        let selected = self.selected();
        if let Some(app) = &mut self.app {
            if app.delete_confirmation().is_some_and(|value| {
                selected.as_ref().is_none_or(|(scope, name)| {
                    *scope != value.scope || !crate::same_name(name, &value.name)
                })
            }) {
                app.cancel_delete();
            }
        }
        self.filling = true;
        SendMessageW(self.entries, LVM_DELETEALLITEMS, 0, 0);
        self.entry_values.clear();
        SetWindowTextW(self.entry, wide("").as_ptr());
        SetWindowTextW(
            self.entry_label,
            wide("Selected &entry — exact text").as_ptr(),
        );
        let selected = selected_index(self.variables).and_then(|i| self.visible.get(i));
        if let (Some(app), Some((scope, name))) = (&self.app, selected) {
            if let Some(value) = app.select(*scope, name).cloned().or_else(|| {
                app.draft(*scope, name).map(|d| {
                    crate::Variable::text(
                        *scope,
                        &d.name,
                        &d.entries.join(";"),
                        d.registry_type == 2,
                    )
                })
            }) {
                let type_name = match value.registry_type {
                    1 => "REG_SZ".into(),
                    2 => "REG_EXPAND_SZ (raw references)".into(),
                    other => format!("registry type {other}"),
                };
                let title = format!(
                    "{} · {}\r\n{}{}",
                    scope_name(*scope),
                    if name.is_empty() {
                        "(unnamed value)"
                    } else {
                        name
                    },
                    type_name,
                    if app.draft(*scope, name).is_some_and(|d| d.base.is_none()) {
                        " - new variable (not yet saved)"
                    } else if app.select(*scope, name).is_none() {
                        " - absent in Windows; draft retained"
                    } else if value.read_only() {
                        " · unsupported name/text/type — read-only"
                    } else {
                        " · saved value"
                    }
                );
                let title = if let Some(draft) = app.draft(*scope, name) {
                    format!(
                        "{title}\r\n{}",
                        if draft.conflict {
                            "Conflict - draft preserved; Discard reloads Windows"
                        } else {
                            "Unsaved draft"
                        }
                    )
                } else {
                    title
                };
                SetWindowTextW(self.detail, wide(&title).as_ptr());
                if let Some(entries) = app
                    .draft(*scope, name)
                    .map(|d| d.entries.clone())
                    .or_else(|| value.entries())
                {
                    self.entry_values = entries;
                    for (i, entry) in self.entry_values.iter().enumerate() {
                        let preview = if entry.is_empty() {
                            "(empty entry)".into()
                        } else {
                            entry.replace('\r', "\\r").replace('\n', "\\n")
                        };
                        insert_row(self.entries, i, &[(i + 1).to_string(), preview]);
                    }
                    select_row(self.entries, 0);
                } else {
                    SetWindowTextW(
                        self.entry_label,
                        wide("Raw registry bytes (hexadecimal) — read-only").as_ptr(),
                    );
                    let bytes = value
                        .bytes
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    SetWindowTextW(self.entry, wide(&bytes).as_ptr());
                }
            }
        } else {
            SetWindowTextW(self.detail, wide("No matching variable selected").as_ptr());
        }
        self.filling = false;
        self.show_entry();
        self.edit_gate();
    }
    unsafe fn show_entry(&self) {
        if let Some(index) = selected_index(self.entries) {
            if let Some(entry) = self.entry_values.get(index) {
                SetWindowTextW(
                    self.entry_label,
                    wide(&format!(
                        "Selected &entry {} — exact text ({} characters)",
                        index + 1,
                        entry.chars().count()
                    ))
                    .as_ptr(),
                );
                SetWindowTextW(self.entry, wide(entry).as_ptr());
            }
        }
    }
    fn selected(&self) -> Option<(Scope, String)> {
        unsafe {
            selected_index(self.variables)
                .and_then(|i| self.visible.get(i))
                .cloned()
        }
    }
    unsafe fn edit_gate(&self) {
        let editable = self.selected().is_some_and(|(scope, name)| {
            self.app.as_ref().is_some_and(|app| {
                app.select(scope, &name).is_some_and(|v| !v.read_only())
                    || app.draft(scope, &name).is_some_and(|d| d.base.is_none())
            })
        });
        let new_draft = self
            .selected()
            .and_then(|(scope, name)| self.app.as_ref().and_then(|app| app.draft(scope, &name)))
            .filter(|d| d.base.is_none());
        ShowWindow(
            self.expand,
            if new_draft.is_some() {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        SendMessageW(
            self.expand,
            BM_SETCHECK,
            if new_draft.is_some_and(|d| d.registry_type == 2) {
                BST_CHECKED as usize
            } else {
                BST_UNCHECKED as usize
            },
            0,
        );
        EnableWindow(self.new_button, self.app.is_some() as i32);
        EnableWindow(self.new_name, self.app.is_some() as i32);
        EnableWindow(self.expand, editable as i32);
        let can_delete = self.selected().is_some_and(|(scope, name)| {
            self.app.as_ref().is_some_and(|app| {
                app.storage_error().is_none()
                    && app.monitoring_errors().is_empty()
                    && [Scope::User, Scope::System]
                        .iter()
                        .all(|s| app.scope_error(*s).is_none())
                    && !app.draft(scope, &name).is_some_and(|d| d.conflict)
                    && app.select(scope, &name).is_some()
                    && editable
            })
        });
        EnableWindow(self.delete, can_delete as i32);
        let confirming = self
            .app
            .as_ref()
            .is_some_and(|app| app.delete_confirmation().is_some());
        ShowWindow(
            self.confirm_delete,
            if confirming { SW_SHOW } else { SW_HIDE },
        );
        ShowWindow(
            self.cancel_delete,
            if confirming { SW_SHOW } else { SW_HIDE },
        );
        EnableWindow(
            self.confirm_delete,
            (confirming
                && can_delete
                && self
                    .app
                    .as_ref()
                    .is_some_and(|app| app.storage_error().is_none())) as i32,
        );
        SendMessageW(self.entry, EM_SETREADONLY, (!editable) as usize, 0);
        for (i, hwnd) in self.buttons.iter().enumerate() {
            let enabled = match i {
                0..=3 => editable,
                7 => self.app.is_some(),
                4 => self.selected().is_some_and(|(scope, name)| {
                    self.app
                        .as_ref()
                        .is_some_and(|app| app.can_save(scope, &name))
                }),
                6 => self.app.is_some(),
                _ => self.app.is_some() && self.selected().is_some(),
            };
            EnableWindow(*hwnd, enabled as i32);
        }
        // Navigation can refresh this gate while the inline Exit choice is open.
        if self.lifecycle.confirming() {
            for control in [
                self.entry,
                self.new_name,
                self.new_button,
                self.expand,
                self.delete,
                self.confirm_delete,
                self.cancel_delete,
                self.startup,
            ] {
                EnableWindow(control, 0);
            }
            for control in &self.buttons {
                EnableWindow(*control, 0);
            }
            ShowWindow(self.confirm_delete, SW_HIDE);
            ShowWindow(self.cancel_delete, SW_HIDE);
        }
    }
    unsafe fn edited(&mut self) {
        if self.filling {
            return;
        }
        let Some(index) = selected_index(self.entries) else {
            return;
        };
        let Some((scope, name)) = self.selected() else {
            return;
        };
        let text = window_text(self.entry);
        if self.entry_values.get(index) == Some(&text) {
            return;
        }
        self.entry_values[index] = text;
        if let Some(app) = &mut self.app {
            match app.edit_entries(scope, &name, self.entry_values.clone()) {
                Ok(()) => {
                    let message = app.draft(scope, &name).and_then(|draft| draft.validation_error()).unwrap_or(
                        "Unsaved draft. Save applies directly; Discard reloads this variable only."
                    );
                    SetWindowTextW(self.status, wide(message).as_ptr());
                }
                Err(error) => {
                    SetWindowTextW(self.status, wide(&error).as_ptr());
                }
            }
        }
        let preview = entry_preview(&self.entry_values[index]);
        let mut text = wide(&preview);
        let item = LVITEMW {
            iItem: index as i32,
            iSubItem: 1,
            pszText: text.as_mut_ptr(),
            ..Default::default()
        };
        SendMessageW(
            self.entries,
            LVM_SETITEMTEXTW,
            index,
            &item as *const _ as isize,
        );
        self.edit_gate();
    }
    unsafe fn entry_action(&mut self, id: usize) {
        let Some((scope, name)) = self.selected() else {
            return;
        };
        let mut row = selected_index(self.entries).unwrap_or(0);
        match id {
            ADD => {
                row = self.entry_values.len();
                self.entry_values.push(String::new());
            }
            REMOVE if row < self.entry_values.len() => {
                self.entry_values.remove(row);
                if self.entry_values.is_empty() {
                    self.entry_values.push(String::new());
                }
                row = row.min(self.entry_values.len() - 1);
            }
            UP if row > 0 => {
                self.entry_values.swap(row, row - 1);
                row -= 1;
            }
            DOWN if row + 1 < self.entry_values.len() => {
                self.entry_values.swap(row, row + 1);
                row += 1;
            }
            _ => return,
        }
        if let Some(app) = &mut self.app {
            if let Err(error) = app.edit_entries(scope, &name, self.entry_values.clone()) {
                SetWindowTextW(self.status, wide(&error).as_ptr());
                return;
            }
        }
        self.show_variable();
        self.filling = true;
        select_row(self.entries, row);
        self.filling = false;
        self.show_entry();
        SetFocus(self.entry);
        SetWindowTextW(
            self.status,
            wide("Unsaved draft. Save applies directly.").as_ptr(),
        );
    }
    unsafe fn refresh(&mut self, kind: RefreshKind) {
        let Some(app) = self.app.take() else {
            return;
        };
        if let Err(error) = self.sender.try_send(Work::Refresh(app, kind)) {
            let work = match error {
                std::sync::mpsc::TrySendError::Full(work)
                | std::sync::mpsc::TrySendError::Disconnected(work) => work,
            };
            if let Work::Refresh(app, _) = work {
                self.app = Some(app);
            }
            let mut monitor = self.monitor.lock().unwrap();
            monitor.changed = true;
            monitor.gap |= !matches!(kind, RefreshKind::Live);
            return;
        }
        for hwnd in [
            self.search,
            self.filter,
            self.variables,
            self.entry,
            self.new_name,
            self.new_button,
            self.expand,
            self.delete,
            self.confirm_delete,
            self.cancel_delete,
        ] {
            EnableWindow(hwnd, 0);
        }
        for hwnd in &self.buttons {
            EnableWindow(*hwnd, 0);
        }
    }
    unsafe fn new_variable(&mut self) {
        let name = window_text(self.new_name);
        let scope = if SendMessageW(self.filter, CB_GETCURSEL, 0, 0) == 2 {
            Scope::System
        } else {
            Scope::User
        };
        let Some(app) = &mut self.app else {
            return;
        };
        if let Err(error) = app.create_variable(scope, &name) {
            SetWindowTextW(self.status, wide(&error).as_ptr());
            return;
        }
        self.filling = true;
        SetWindowTextW(self.search, wide("").as_ptr());
        SendMessageW(
            self.filter,
            CB_SETCURSEL,
            if scope == Scope::System { 2 } else { 1 },
            0,
        );
        self.filling = false;
        self.populate();
        let scope_for_new = scope;
        if let Some(row) = self
            .visible
            .iter()
            .position(|(scope, value)| *scope == scope_for_new && crate::same_name(value, &name))
        {
            self.filling = true;
            select_row(self.variables, row);
            self.filling = false;
        }
        self.show_variable();
        SetWindowTextW(
            self.status,
            wide(&format!(
                "New {} variable draft; Save creates it{}. Discard cancels only this draft.",
                scope_name(scope),
                if scope == Scope::System {
                    " after administrator approval"
                } else {
                    ""
                }
            ))
            .as_ptr(),
        );
        SetFocus(self.entry);
    }
    unsafe fn deletion(&mut self, confirm: bool) {
        let selected = self.selected();
        let Some(app) = &mut self.app else {
            return;
        };
        if confirm {
            if let Some((scope, name)) = selected {
                let message = match app.request_delete(scope, &name) {
                    Ok(()) => format!("Delete {} variable {name}? Its selected draft will also be discarded. Confirm or cancel inline.{}", scope_name(scope), if scope == Scope::System { " Administrator approval is required." } else { "" }),
                    Err(error) => error,
                };
                SetWindowTextW(self.status, wide(&message).as_ptr());
            }
        } else {
            app.cancel_delete();
            SetWindowTextW(
                self.status,
                wide("Deletion cancelled; values and drafts preserved.").as_ptr(),
            );
        }
        self.edit_gate();
        if confirm {
            SetFocus(self.cancel_delete);
        } else {
            SetFocus(self.delete);
        }
    }
    unsafe fn action(&mut self, id: usize) {
        if id == REFRESH {
            self.refresh(RefreshKind::Recover);
            return;
        }
        if id == HISTORY {
            if let Some(app) = &self.app {
                if let Err(error) = crate::history_window::open(
                    GetParent(self.status),
                    app.data_directory().to_owned(),
                    self.selected(),
                ) {
                    SetWindowTextW(self.status, wide(&error).as_ptr());
                }
            }
            return;
        }
        let Some((scope, name)) = self.selected() else {
            return;
        };
        let Some(mut app) = self.app.take() else {
            return;
        };
        if id == SAVE || id == DISCARD {
            app.cancel_delete();
        }
        let work = match id {
            SAVE => Work::Save(app, scope, name),
            DISCARD => Work::Discard(app, scope, name),
            CONFIRM_DELETE => Work::Delete(app),
            _ => return,
        };
        self.submit(work);
    }
    unsafe fn submit(&mut self, work: Work) {
        if let Err(error) = self.sender.try_send(work) {
            let work = match error {
                std::sync::mpsc::TrySendError::Full(work)
                | std::sync::mpsc::TrySendError::Disconnected(work) => work,
            };
            self.app = Some(match work {
                Work::Startup(app, ..)
                | Work::StartupStatus(app)
                | Work::Save(app, ..)
                | Work::Discard(app, ..)
                | Work::Delete(app)
                | Work::Refresh(app, ..) => app,
            });
            SetWindowTextW(
                self.status,
                wide("Data worker unavailable; draft preserved.").as_ptr(),
            );
            return;
        }
        for hwnd in [
            self.search,
            self.filter,
            self.variables,
            self.entry,
            self.new_name,
            self.new_button,
            self.expand,
            self.delete,
            self.confirm_delete,
            self.cancel_delete,
        ] {
            EnableWindow(hwnd, 0);
        }
        for hwnd in &self.buttons {
            EnableWindow(*hwnd, 0);
        }
        SetWindowTextW(
            self.status,
            wide("Working; drafts are preserved...").as_ptr(),
        );
    }
    unsafe fn update_tray(&mut self, parent: HWND, recover: bool) -> bool {
        if let Some(app) = &self.app {
            self.recording_interrupted = app.storage_error().is_some()
                || !app.monitoring_errors().is_empty()
                || [Scope::User, Scope::System]
                    .iter()
                    .any(|s| app.scope_error(*s).is_some());
        }
        let interrupted =
            self.recording_interrupted || !self.monitor.lock().unwrap().errors.is_empty();
        let ok = self.tray.as_mut().is_some_and(|tray| {
            if recover {
                tray.recover(interrupted)
            } else {
                tray.update(interrupted)
            }
        });
        if !ok && !self.lifecycle.visible() {
            self.reopen(parent);
            SetWindowTextW(
                self.status,
                wide("Tray unavailable; keep this window open for monitoring or choose Exit.")
                    .as_ptr(),
            );
        }
        ok
    }
    unsafe fn reopen(&mut self, parent: HWND) {
        self.lifecycle.reopen();
        ShowWindow(parent, SW_RESTORE);
        SetForegroundWindow(parent);
    }
    unsafe fn reopen_with_status(&mut self, parent: HWND) {
        self.reopen(parent);
        if let Some(app) = self.app.take() {
            self.submit(Work::StartupStatus(app));
        }
    }
    unsafe fn close_to_tray(&mut self, parent: HWND) {
        if self.lifecycle.exit_requested() {
            self.cancel_exit();
        }
        let available = self.update_tray(parent, false);
        if self.lifecycle.close_to_tray(available) {
            crate::history_window::close_all();
            ShowWindow(parent, SW_HIDE);
        } else {
            SetWindowTextW(
                self.status,
                wide("Tray unavailable; keep this window open for monitoring or choose Exit.")
                    .as_ptr(),
            );
        }
    }
    unsafe fn cancel_exit(&mut self) {
        self.lifecycle.cancel_exit();
        ShowWindow(self.exit, SW_SHOW);
        ShowWindow(self.confirm_exit, SW_HIDE);
        ShowWindow(self.cancel_exit, SW_HIDE);
        self.edit_gate();
        EnableWindow(self.entry, self.app.is_some() as i32);
        EnableWindow(self.startup, self.app.is_some() as i32);
        for control in &self.buttons {
            if GetDlgCtrlID(*control) == HISTORY as i32 || GetDlgCtrlID(*control) == REFRESH as i32
            {
                EnableWindow(*control, self.app.is_some() as i32);
            }
        }
        SetWindowTextW(
            self.status,
            wide("Exit cancelled; monitoring and every draft continue.").as_ptr(),
        );
    }
    unsafe fn exit_prompt(&mut self) {
        ShowWindow(self.confirm_exit, SW_SHOW);
        ShowWindow(self.cancel_exit, SW_SHOW);
        ShowWindow(self.exit, SW_HIDE);
        ShowWindow(self.confirm_delete, SW_HIDE);
        ShowWindow(self.cancel_delete, SW_HIDE);
        EnableWindow(self.confirm_exit, self.app.is_some() as i32);
        for control in [
            self.entry,
            self.new_name,
            self.new_button,
            self.expand,
            self.delete,
            self.confirm_delete,
            self.cancel_delete,
            self.startup,
        ] {
            EnableWindow(control, 0);
        }
        for control in &self.buttons {
            EnableWindow(*control, 0);
        }
        SetWindowTextW(
            self.status,
            wide("Unsaved edits exist. Exit and discard unsaved edits, or Cancel exit.").as_ptr(),
        );
    }
    unsafe fn finish_exit(&mut self, parent: HWND) {
        crate::history_window::close_all();
        if !crate::history_window::workers_settled() {
            self.reopen(parent);
            SetWindowTextW(
                self.status,
                wide("Finishing the history operation before exit...").as_ptr(),
            );
            return;
        }
        KillTimer(parent, LOAD_TIMER);
        DestroyWindow(parent);
        PostQuitMessage(0);
    }
    unsafe fn request_exit(&mut self, parent: HWND) {
        if self.worker_stopped && self.app.is_none() {
            self.finish_exit(parent);
            return;
        }
        match self.lifecycle.request_exit(self.app.as_ref()) {
            ExitDecision::WaitForOperation => {
                self.reopen(parent);
                SetWindowTextW(self.status,wide("Finishing the current operation before exit; saved changes and pending recovery are preserved.").as_ptr());
            }
            ExitDecision::ConfirmDiscard => {
                self.reopen(parent);
                self.exit_prompt();
                SetFocus(self.cancel_exit);
            }
            ExitDecision::Exit => {
                self.finish_exit(parent);
            }
        }
    }
    unsafe fn startup_setting(&mut self) {
        let enabled = SendMessageW(self.startup, BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
        let Some(app) = self.app.take() else { return };
        EnableWindow(self.startup, 0);
        self.submit(Work::Startup(app, enabled));
    }
}

unsafe fn child(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    extended: u32,
    id: usize,
) -> HWND {
    CreateWindowExW(
        extended,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        0,
        0,
        0,
        0,
        parent,
        id as HMENU,
        GetModuleHandleW(null()),
        null(),
    )
}
fn entry_preview(entry: &str) -> String {
    if entry.is_empty() {
        "(empty entry)".into()
    } else {
        entry.replace('\r', "\\r").replace('\n', "\\n")
    }
}
unsafe fn configure_list(hwnd: HWND, columns: &[&str]) {
    SendMessageW(
        hwnd,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        0,
        (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
    );
    for (index, title) in columns.iter().enumerate() {
        let mut text = wide(title);
        let column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: 180,
            pszText: text.as_mut_ptr(),
            ..Default::default()
        };
        SendMessageW(hwnd, LVM_INSERTCOLUMNW, index, &column as *const _ as isize);
    }
}
unsafe fn insert_row(hwnd: HWND, index: usize, cells: &[String]) {
    for (column, value) in cells.iter().enumerate() {
        let mut text = wide(value);
        let item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: index as i32,
            iSubItem: column as i32,
            pszText: text.as_mut_ptr(),
            ..Default::default()
        };
        SendMessageW(
            hwnd,
            if column == 0 {
                LVM_INSERTITEMW
            } else {
                LVM_SETITEMTEXTW
            },
            if column == 0 { 0 } else { index },
            &item as *const _ as isize,
        );
    }
}
unsafe fn select_row(hwnd: HWND, index: usize) {
    let item = LVITEMW {
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        state: LVIS_SELECTED | LVIS_FOCUSED,
        ..Default::default()
    };
    SendMessageW(hwnd, LVM_SETITEMSTATE, index, &item as *const _ as isize);
}
unsafe fn selected_index(hwnd: HWND) -> Option<usize> {
    let index = SendMessageW(hwnd, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    (index >= 0).then_some(index as usize)
}
unsafe fn window_text(hwnd: HWND) -> String {
    let mut text = vec![0u16; GetWindowTextLengthW(hwnd) as usize + 1];
    let len = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32);
    String::from_utf16_lossy(&text[..len as usize])
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<WindowState>;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    // SendMessage and window creation can synchronously re-enter this procedure.
    // Ignore nested state notifications until the outer update is consistent.
    let Ok(mut state) = (&*pointer).try_borrow_mut() else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };
    if state
        .tray
        .as_ref()
        .is_some_and(|tray| tray.taskbar_message() != 0 && message == tray.taskbar_message())
    {
        state.update_tray(hwnd, true);
        return 0;
    }
    match message {
        WM_CLOSE => {
            state.close_to_tray(hwnd);
            0
        }
        native_lifecycle::REOPEN_MESSAGE => {
            state.reopen_with_status(hwnd);
            0
        }
        native_lifecycle::TRAY_MESSAGE => {
            let command = match (lparam as u32) & 0xffff {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK | 1024 | 1025 => native_lifecycle::REOPEN,
                WM_RBUTTONUP | WM_CONTEXTMENU => state.tray.as_ref().map_or(0, |tray| tray.menu()),
                _ => 0,
            };
            if command == native_lifecycle::REOPEN {
                state.reopen_with_status(hwnd);
            }
            if command == native_lifecycle::EXIT {
                state.request_exit(hwnd);
            }
            0
        }
        WM_CREATE => {
            state.create(hwnd);
            0
        }
        WM_SIZE => {
            state.layout(hwnd);
            0
        }
        WM_TIMER if wparam == LOAD_TIMER => {
            state.loaded(hwnd);
            0
        }
        WM_POWERBROADCAST => {
            let recognized = state.monitor.lock().unwrap().power_event(wparam as u32);
            if recognized {
                let errors = state.monitor.lock().unwrap().errors.clone();
                if let Some(app) = &mut state.app {
                    app.set_monitoring_errors(errors);
                }
                state.populate();
                state.update_tray(hwnd, false);
                return 1;
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_SETTINGCHANGE => {
            let mut monitor = state.monitor.lock().unwrap();
            monitor.changed = true;
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            let event = (wparam >> 16) & 0xffff;
            if event == BN_CLICKED as usize {
                match id {
                    native_lifecycle::EXIT => {
                        state.request_exit(hwnd);
                        return 0;
                    }
                    CANCEL_EXIT => {
                        state.cancel_exit();
                        return 0;
                    }
                    CONFIRM_EXIT => {
                        if state.lifecycle.confirming() {
                            if let Some(mut app) = state.app.take() {
                                state.lifecycle.confirm_exit(&mut app);
                                state.app = Some(app);
                                state.finish_exit(hwnd);
                            } else {
                                SetWindowTextW(state.status,wide("Finishing the current operation; try exit again when it completes.").as_ptr());
                            }
                        }
                        return 0;
                    }
                    STARTUP => {
                        state.startup_setting();
                        return 0;
                    }
                    _ => {}
                }
            }
            if id == NEW && event == BN_CLICKED as usize {
                state.new_variable();
            }
            if id == EXPAND && event == BN_CLICKED as usize {
                if let Some((scope, name)) = state.selected() {
                    let expand =
                        SendMessageW(state.expand, BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
                    if let Some(app) = &mut state.app {
                        let _ = app.set_new_expand_in(scope, &name, expand);
                    }
                    state.edit_gate();
                }
            }
            if id == DELETE && event == BN_CLICKED as usize {
                state.deletion(true);
            }
            if id == CANCEL_DELETE && event == BN_CLICKED as usize {
                state.deletion(false);
            }
            if id == CONFIRM_DELETE && event == BN_CLICKED as usize {
                state.action(id);
            }
            if id == ENTRY && event == EN_CHANGE as usize {
                state.edited();
            }
            if (ADD..=DOWN).contains(&id) && event == BN_CLICKED as usize {
                state.entry_action(id);
            }
            if (SAVE..=REFRESH).contains(&id) && event == BN_CLICKED as usize {
                state.action(id);
            }
            if (id == SEARCH && event == EN_CHANGE as usize)
                || (id == FILTER && event == CBN_SELCHANGE as usize)
            {
                state.populate();
            }
            0
        }
        WM_NOTIFY => {
            let notification = &*(lparam as *const NMHDR);
            if !state.filling && notification.code == LVN_ITEMCHANGED {
                if notification.idFrom == VARIABLES {
                    state.show_variable();
                }
                if notification.idFrom == ENTRIES {
                    state.show_entry();
                }
            }
            0
        }
        WM_DPICHANGED => {
            state.dpi = (wparam & 0xffff) as u32;
            state.set_font();
            let rect = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            state.layout(hwnd);
            0
        }
        WM_GETMINMAXINFO => {
            let size = &mut *(lparam as *mut MINMAXINFO);
            size.ptMinTrackSize = POINT {
                x: 800 * state.dpi as i32 / 96,
                y: 560 * state.dpi as i32 / 96,
            };
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, LOAD_TIMER);
            PostQuitMessage(0);
            0
        }
        _ => {
            drop(state);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }
}
/// Owns a single native window. Registry and SQLite initialization run off the UI
/// thread. Commands and responses use bounded channels; values are never logged.
pub fn run(
    reader: impl RegistryWriter + Send + 'static,
    data_directory: PathBuf,
) -> Result<(), String> {
    run_with_startup(
        reader,
        data_directory,
        WindowsStartup::current_user(),
        std::env::current_exe().map_err(|e| e.to_string())?,
        std::env::args().any(|arg| arg == "--tray"),
    )
}
/// Developer fixtures supply an owned startup adapter; ordinary smoke must use it.
pub fn run_with_startup(
    reader: impl RegistryWriter + Send + 'static,
    data_directory: PathBuf,
    registration: impl StartupRegistration + Send + 'static,
    executable: PathBuf,
    start_in_tray: bool,
) -> Result<(), String> {
    crate::system_libraries::init_controls()?;
    let (sender, receiver) = sync_channel(1);
    let (window_ready_sender, window_ready_receiver) = sync_channel(1);
    let (work_sender, work_receiver) = sync_channel(1);
    let monitor = std::sync::Arc::new(std::sync::Mutex::new(crate::resume::Monitoring::default()));
    let worker_monitor = monitor.clone();
    let worker = std::thread::Builder::new()
        .name("environment-storage".into())
        .spawn(move || {
            let mut watch = reader.watcher();
            worker_monitor.lock().unwrap().poll(watch.as_mut());
            let mut app = Application::open(&reader, &data_directory);
            app.set_monitoring_errors(worker_monitor.lock().unwrap().errors.clone());
            if window_ready_receiver.recv().is_err() {
                return;
            }
            let mut startup = Startup::initialize(&app, &registration, &executable);
            let startup_state = Some(startup.as_ref().map(|s| s.enabled()).map_err(Clone::clone));
            if sender
                .send(Response {
                    app,
                    message: None,
                    startup: startup_state,
                })
                .is_err()
            {
                return;
            }
            loop {
                worker_monitor.lock().unwrap().poll(watch.as_mut());
                let work = match work_receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(work) => work,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let (monitor_errors, power_gap) = {
                    let mut monitor = worker_monitor.lock().unwrap();
                    monitor.poll(watch.as_mut());
                    (monitor.errors.clone(), monitor.gap)
                };
                let mut response = match work {
                    Work::StartupStatus(app) => Response {
                        app,
                        message: None,
                        startup: None,
                    },
                    Work::Startup(app, enabled) => {
                        if startup.is_err() {
                            startup = Startup::initialize(&app, &registration, &executable);
                        }
                        let result = match &mut startup {
                            Ok(s) => s.set_enabled(enabled, &registration),
                            Err(e) => Err(e.clone()),
                        };
                        Response {
                            app,
                            message: Some(match result {
                                Ok(()) => "Sign-in startup preference saved.".into(),
                                Err(e) => e,
                            }),
                            startup: None,
                        }
                    }
                    Work::Refresh(mut app, kind) => {
                        app.set_monitoring_errors(monitor_errors);
                        // A queued live signal must not classify a power gap as live.
                        let kind = if power_gap && !matches!(kind, RefreshKind::Recover) {
                            RefreshKind::Gap
                        } else {
                            kind
                        };
                        match kind {
                            RefreshKind::Recover => app.refresh(&reader),
                            RefreshKind::Gap => app.observe_gap(&reader),
                            RefreshKind::Live => app.observe(&reader),
                        }
                        Response {
                            app,
                            message: None,
                            startup: None,
                        }
                    }
                    Work::Save(mut app, scope, name) => {
                        app.set_monitoring_errors(monitor_errors);
                        if power_gap {
                            app.observe_gap(&reader);
                        }
                        let result = app.save(&reader, scope, &name);
                        Response {
                            startup: None,
                            app,
                            message: Some(match result {
                                Ok(crate::SaveOutcome::Saved) => {
                                    "Saved and recorded in history.".into()
                                }
                                Ok(crate::SaveOutcome::NoChange) => {
                                    "No changes; Windows was not rewritten.".into()
                                }
                                Err(error) => error,
                            }),
                        }
                    }
                    Work::Delete(mut app) => {
                        app.set_monitoring_errors(monitor_errors);
                        if power_gap {
                            app.observe_gap(&reader);
                        }
                        let result = app.confirm_delete(&reader);
                        Response {
                            startup: None,
                            app,
                            message: Some(match result {
                                Ok(_) => "Deleted and recorded in history.".into(),
                                Err(error) => error,
                            }),
                        }
                    }
                    Work::Discard(mut app, scope, name) => {
                        let result = app.discard(&reader, scope, &name);
                        Response {
                            startup: None,
                            app,
                            message: Some(match result {
                                Ok(()) => {
                                    "Selected draft discarded; current Windows value reloaded."
                                        .into()
                                }
                                Err(error) => error,
                            }),
                        }
                    }
                };
                if startup.is_err() {
                    startup = Startup::initialize(&response.app, &registration, &executable);
                }
                response.startup = Some(match &mut startup {
                    Ok(s) => s.refresh_status(&registration).map(|()| s.enabled()),
                    Err(error) => Err(error.clone()),
                });
                if sender.send(response).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| format!("Cannot start data worker: {e}"))?;
    let mut state = Box::new(RefCell::new(WindowState::new(
        receiver,
        work_sender,
        monitor,
    )));
    let result = unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let class = wide("EnvironmentManager.Main");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hIcon: crate::icons::application_icon(false),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE + 1) as HBRUSH,
            ..Default::default()
        };
        RegisterClassW(&window_class);
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            wide("Environment Manager — Loading").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1050,
            700,
            null_mut(),
            null_mut(),
            instance,
            &mut *state as *mut _ as *const _,
        );
        if hwnd.is_null() {
            Err(format!(
                "Cannot create native window (Windows error {})",
                GetLastError()
            ))
        } else {
            let _ = window_ready_sender.send(());
            ShowWindow(hwnd, SW_SHOW);
            if start_in_tray {
                state.borrow_mut().close_to_tray(hwnd);
            }
            UpdateWindow(hwnd);
            let mut message = MSG::default();
            let mut outcome = Ok(());
            loop {
                let next = GetMessageW(&mut message, null_mut(), 0, 0);
                if next == 0 {
                    break;
                }
                if next == -1 {
                    outcome = Err("Native message loop failed".into());
                    break;
                }
                if IsDialogMessageW(GetAncestor(message.hwnd, GA_ROOT), &message) == 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
            if !state.borrow().font.is_null() {
                DeleteObject(state.borrow().font);
            }
            outcome
        }
    };
    drop(window_ready_sender);
    drop(state);
    let _ = worker.join();
    result
}
