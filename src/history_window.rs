//! Separate paged history window. Only one database request can be in flight.
use crate::{
    history_comparison::{Comparison, EntryChange},
    storage, HistoryRecord, Scope,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    ptr::{null, null_mut},
    sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::*,
        HiDpi::*,
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::*,
    },
};
const CLASS: &str = "EnvironmentManager.PagedHistory";
thread_local! {
    // Only the creating UI thread may read this HWND or its local state.
    static OWNED_WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
    static FINISHING_WORKERS: RefCell<Vec<std::thread::JoinHandle<()>>> = const { RefCell::new(Vec::new()) };
}
const FILTER: usize = 201;
const RECORDS: usize = 202;
const ROWS: usize = 203;
const PREVIOUS: usize = 204;
const NEXT: usize = 205;
const CLEAR: usize = 206;
const CONFIRM: usize = 207;
const CANCEL: usize = 208;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn scope(s: Scope) -> &'static str {
    if s == Scope::User {
        "User"
    } else {
        "System"
    }
}
fn preview(s: &str) -> String {
    format!("{s:?}").replace("\\\\", "\\")
}
enum Request {
    Page(Option<(Scope, String)>, usize),
    Clear(Option<(Scope, String)>),
}
struct Reply {
    result: Result<Vec<HistoryRecord>, String>,
    cleared: Option<usize>,
}
struct State {
    worker: Option<std::thread::JoinHandle<()>>,
    sender: SyncSender<Request>,
    receiver: Receiver<Reply>,
    selected: Option<(Scope, String)>,
    offset: usize,
    busy: bool,
    confirming: bool,
    records: Vec<HistoryRecord>,
    comparison: Option<Comparison>,
    font: HFONT,
    dpi: u32,
    filter: HWND,
    list: HWND,
    rows: HWND,
    status: HWND,
    before: HWND,
    after: HWND,
    controls: Vec<HWND>,
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            FINISHING_WORKERS.with_borrow_mut(|workers| workers.push(worker));
        }
        // Dropping the channels ends the worker after its accepted request finishes.
    }
}
/// Nonblocking shutdown handshake; a closed history window may still be clearing.
/// The UI thread must poll this before terminating its process.
pub fn workers_settled() -> bool {
    FINISHING_WORKERS.with_borrow_mut(|workers| {
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        workers.is_empty()
    })
}
unsafe fn child(parent: HWND, class: &str, text: &str, style: u32, id: usize) -> HWND {
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
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
unsafe fn columns(hwnd: HWND, titles: &[(&str, i32)]) {
    SendMessageW(
        hwnd,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        0,
        (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
    );
    for (i, (title, width)) in titles.iter().enumerate() {
        let mut text = wide(title);
        let column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: *width,
            pszText: text.as_mut_ptr(),
            ..Default::default()
        };
        SendMessageW(hwnd, LVM_INSERTCOLUMNW, i, &column as *const _ as isize);
    }
}
unsafe fn row(hwnd: HWND, index: usize, cells: &[String]) {
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
unsafe fn index(hwnd: HWND) -> Option<usize> {
    let n = SendMessageW(hwnd, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    (n >= 0).then_some(n as usize)
}
unsafe fn select(hwnd: HWND) {
    let item = LVITEMW {
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        state: LVIS_SELECTED | LVIS_FOCUSED,
        ..Default::default()
    };
    SendMessageW(hwnd, LVM_SETITEMSTATE, 0, &item as *const _ as isize);
}
impl State {
    unsafe fn create(&mut self, hwnd: HWND) {
        self.dpi = GetDpiForWindow(hwnd).max(96);
        self.filter = child(
            hwnd,
            "COMBOBOX",
            "History filter",
            WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
            FILTER,
        );
        let label = self
            .selected
            .as_ref()
            .map(|(s, n)| format!("Selected variable: {} / {}", scope(*s), n))
            .unwrap_or_else(|| "All variables (no selection)".into());
        for text in [label, "All variables / both scopes".into()] {
            SendMessageW(self.filter, CB_ADDSTRING, 0, wide(&text).as_ptr() as isize);
        }
        SendMessageW(
            self.filter,
            CB_SETCURSEL,
            if self.selected.is_some() { 0 } else { 1 },
            0,
        );
        self.list = child(
            hwnd,
            "SysListView32",
            "History records: scope, name, source, outcome and UTC observation time",
            WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
            RECORDS,
        );
        columns(
            self.list,
            &[
                ("Scope", 70),
                ("Variable", 170),
                ("Source", 180),
                ("Outcome", 100),
                ("Observed UTC epoch ms", 170),
                ("Record", 70),
            ],
        );
        self.rows = child(
            hwnd,
            "SysListView32",
            "Before and After value entries. Comparison labels infer snapshot differences",
            WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
            ROWS,
        );
        columns(
            self.rows,
            &[
                ("Before #", 65),
                ("Before label", 90),
                ("Before entry (quoted exact text)", 290),
                ("After #", 65),
                ("After label", 90),
                ("After entry (quoted exact text)", 290),
            ],
        );
        self.before = child(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            209,
        );
        self.after = child(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            210,
        );
        for edit in [self.before, self.after] {
            SendMessageW(edit, EM_SETLIMITTEXT, 16 * 1024 * 1024, 0);
        }
        self.status = child(
            hwnd,
            "STATIC",
            "Observed snapshots; external actor/time and intermediate operations may be unknown.",
            0,
            0,
        );
        for (id, text) in [
            (PREVIOUS, "&Previous page"),
            (NEXT, "&Next page"),
            (CLEAR, "C&lear history..."),
            (CONFIRM, "&Clear completed history"),
            (CANCEL, "Ca&ncel clear"),
        ] {
            self.controls.push(child(
                hwnd,
                "BUTTON",
                text,
                WS_TABSTOP | BS_PUSHBUTTON as u32,
                id,
            ));
        }
        self.set_font();
        self.layout(hwnd);
        self.refresh_controls();
        SetTimer(hwnd, 1, 40, None);
        self.request(false);
        SetFocus(self.filter);
    }
    unsafe fn set_font(&mut self) {
        let old = self.font;
        self.font = CreateFontW(
            -(10 * self.dpi as i32 / 72),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            0,
            0,
            wide("Segoe UI").as_ptr(),
        );
        for hwnd in [
            self.filter,
            self.list,
            self.rows,
            self.status,
            self.before,
            self.after,
        ]
        .into_iter()
        .chain(self.controls.iter().copied())
        {
            SendMessageW(hwnd, WM_SETFONT, self.font as usize, 1);
        }
        if !old.is_null() {
            DeleteObject(old);
        }
    }
    unsafe fn layout(&self, hwnd: HWND) {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        let scale = |n: i32| n * self.dpi as i32 / 96;
        let margin = scale(10);
        let width = rect.right - 2 * margin;
        let bottom = scale(37);
        let status = scale(50);
        let filter = scale(30);
        let remaining = (rect.bottom - filter - bottom - status - 4 * margin).max(scale(180));
        let list = remaining / 3;
        let rows = remaining / 2;
        let detail = remaining - list - rows;
        MoveWindow(self.filter, margin, margin, width, scale(250), 1);
        MoveWindow(self.list, margin, margin + filter, width, list, 1);
        MoveWindow(self.rows, margin, margin + filter + list, width, rows, 1);
        MoveWindow(
            self.before,
            margin,
            margin + filter + list + rows,
            width / 2,
            detail,
            1,
        );
        MoveWindow(
            self.after,
            margin + width / 2,
            margin + filter + list + rows,
            width - width / 2,
            detail,
            1,
        );
        MoveWindow(
            self.status,
            margin,
            rect.bottom - bottom - status,
            width,
            status,
            1,
        );
        for (i, control) in self.controls.iter().enumerate() {
            let widths = [130, 130, 140, 210, 140];
            let x = widths[..i].iter().sum::<i32>();
            MoveWindow(
                *control,
                margin + scale(x),
                rect.bottom - bottom,
                scale(widths[i]),
                scale(28),
                1,
            );
        }
    }
    unsafe fn active_filter(&self) -> Option<(Scope, String)> {
        if SendMessageW(self.filter, CB_GETCURSEL, 0, 0) == 0 {
            self.selected.clone()
        } else {
            None
        }
    }
    unsafe fn refresh_controls(&self) {
        EnableWindow(self.filter, (!self.busy && !self.confirming) as i32);
        for (i, button) in self.controls.iter().enumerate() {
            ShowWindow(
                *button,
                if (i >= 3) == self.confirming {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
            EnableWindow(
                *button,
                (!self.busy && (i != 0 || self.offset > 0) && (i != 1 || self.records.len() == 50))
                    as i32,
            );
        }
    }
    unsafe fn request(&mut self, clear: bool) {
        if self.busy {
            return;
        }
        let filter = self.active_filter();
        let request = if clear {
            Request::Clear(filter)
        } else {
            Request::Page(filter, self.offset)
        };
        if self.sender.try_send(request).is_err() {
            SetWindowTextW(
                self.status,
                wide("History worker unavailable. No clear was performed.").as_ptr(),
            );
            return;
        }
        self.busy = true;
        self.confirming = false;
        self.refresh_controls();
        SetWindowTextW(
            self.status,
            wide("Loading history... Windows values and drafts are unchanged.").as_ptr(),
        );
    }
    unsafe fn loaded(&mut self) {
        match self.receiver.try_recv() {
            Ok(reply) => {
                self.busy = false;
                match reply.result {
                    Ok(records) => {
                        self.records = records;
                        self.comparison = None;
                        SendMessageW(self.list, LVM_DELETEALLITEMS, 0, 0);
                        SendMessageW(self.rows, LVM_DELETEALLITEMS, 0, 0);
                        SetWindowTextW(self.before, wide("").as_ptr());
                        SetWindowTextW(self.after, wide("").as_ptr());
                        for (i, r) in self.records.iter().enumerate() {
                            row(
                                self.list,
                                i,
                                &[
                                    scope(r.scope).into(),
                                    r.name.clone(),
                                    r.source.clone(),
                                    r.outcome.clone(),
                                    r.time_ms.to_string(),
                                    r.id.to_string(),
                                ],
                            );
                        }
                        if !self.records.is_empty() {
                            select(self.list);
                            self.show_record();
                        }
                        let text = if let Some(count) = reply.cleared {
                            format!("Cleared {count} completed records for this filter. Current values, baseline, pending recovery and backups preserved.")
                        } else {
                            format!("Records {}-{}. Select a record, then an entry. Quoted text preserves whitespace/empty entries; raw selected entry below. Labels infer snapshots, not a writer's operation sequence.",if self.records.is_empty(){0}else{self.offset+1},self.offset+self.records.len())
                        };
                        SetWindowTextW(self.status, wide(&text).as_ptr());
                    }
                    Err(error) => {
                        SetWindowTextW(self.status, wide(&error).as_ptr());
                    }
                }
                self.refresh_controls();
            }
            Err(TryRecvError::Disconnected) => {
                self.busy = false;
                self.refresh_controls();
                SetWindowTextW(
                    self.status,
                    wide("History worker stopped; close and reopen history.").as_ptr(),
                );
            }
            Err(TryRecvError::Empty) => {}
        }
    }
    unsafe fn show_record(&mut self) {
        let Some(record) = index(self.list).and_then(|i| self.records.get(i)) else {
            return;
        };
        let view = record.comparison();
        SendMessageW(self.rows, LVM_DELETEALLITEMS, 0, 0);
        for (i, r) in view.rows.iter().enumerate() {
            let before_label = if r.change == EntryChange::Deleted {
                "Deleted"
            } else if r.before.is_some() {
                "Before"
            } else {
                "-"
            };
            let after_label = match r.change {
                EntryChange::Added => "Added",
                EntryChange::Edited => "Edited",
                EntryChange::Unchanged => "Unchanged",
                EntryChange::Deleted => "-",
            };
            row(
                self.rows,
                i,
                &[
                    r.before
                        .as_ref()
                        .map(|v| v.number.to_string())
                        .unwrap_or_default(),
                    before_label.into(),
                    r.before
                        .as_ref()
                        .map(|v| preview(&v.text))
                        .unwrap_or_default(),
                    r.after
                        .as_ref()
                        .map(|v| v.number.to_string())
                        .unwrap_or_default(),
                    after_label.into(),
                    r.after
                        .as_ref()
                        .map(|v| preview(&v.text))
                        .unwrap_or_default(),
                ],
            );
        }
        let description = |present: bool, raw: &Option<Vec<u8>>, kind: Option<u32>| {
            if !present {
                "Absent variable".into()
            } else if let Some(bytes) = raw {
                format!("Unsupported raw registry type {kind:?}: {bytes:02X?}")
            } else {
                format!("Present; registry type {kind:?}. Select an entry to inspect exact text.")
            }
        };
        SetWindowTextW(
            self.before,
            wide(&description(
                view.before_present,
                &view.before_raw,
                record.before.as_ref().map(|v| v.registry_type),
            ))
            .as_ptr(),
        );
        SetWindowTextW(
            self.after,
            wide(&description(
                view.after_present,
                &view.after_raw,
                record.after.as_ref().map(|v| v.registry_type),
            ))
            .as_ptr(),
        );
        self.comparison = Some(view);
    }
    unsafe fn show_entry(&self) {
        if let Some(row) = index(self.rows).and_then(|i| self.comparison.as_ref()?.rows.get(i)) {
            SetWindowTextW(
                self.before,
                wide(
                    row.before
                        .as_ref()
                        .map(|v| v.text.as_str())
                        .unwrap_or("(no Before entry)"),
                )
                .as_ptr(),
            );
            SetWindowTextW(
                self.after,
                wide(
                    row.after
                        .as_ref()
                        .map(|v| v.text.as_str())
                        .unwrap_or("(no After entry)"),
                )
                .as_ptr(),
            );
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(
                (&mut *((*(l as *const CREATESTRUCTW)).lpCreateParams
                    as *mut Option<Box<RefCell<State>>>))
                    .take()
                    .expect("history creation owns state"),
            ) as isize,
        );
    }
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<State>;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    if message == WM_NCDESTROY {
        OWNED_WINDOW.set(null_mut());
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let state = Box::from_raw(pointer);
        if !state.borrow().font.is_null() {
            DeleteObject(state.borrow().font);
        }
        drop(state);
        return DefWindowProcW(hwnd, message, w, l);
    }
    let Ok(mut state) = (&*pointer).try_borrow_mut() else {
        return DefWindowProcW(hwnd, message, w, l);
    };
    match message {
        WM_CREATE => {
            state.create(hwnd);
            0
        }
        WM_SIZE => {
            state.layout(hwnd);
            0
        }
        WM_TIMER => {
            state.loaded();
            0
        }
        WM_COMMAND => {
            let id = w & 0xffff;
            let event = (w >> 16) & 0xffff;
            if id == FILTER && event == CBN_SELCHANGE as usize {
                state.offset = 0;
                state.confirming = false;
                state.request(false);
            }
            if event == BN_CLICKED as usize {
                match id {
                    PREVIOUS if !state.busy => {
                        state.offset = state.offset.saturating_sub(50);
                        state.request(false);
                    }
                    NEXT if !state.busy => {
                        state.offset += 50;
                        state.request(false);
                    }
                    CLEAR => {
                        state.confirming = true;
                        state.refresh_controls();
                        SetWindowTextW(state.status,wide("Clear completed history for the active filter? This cannot be undone. Windows values, drafts, baseline, pending recovery and migration backups are kept.").as_ptr());
                        SetFocus(state.controls[4]);
                    }
                    CONFIRM => {
                        state.offset = 0;
                        state.request(true);
                    }
                    CANCEL => {
                        state.confirming = false;
                        state.refresh_controls();
                        SetWindowTextW(
                            state.status,
                            wide("Clear cancelled. History retained.").as_ptr(),
                        );
                        SetFocus(state.controls[2]);
                    }
                    _ => {}
                }
            }
            0
        }
        WM_NOTIFY => {
            let note = &*(l as *const NMHDR);
            if note.code == NM_CUSTOMDRAW && note.idFrom == ROWS {
                let draw = &mut *(l as *mut NMLVCUSTOMDRAW);
                match draw.nmcd.dwDrawStage {
                    CDDS_PREPAINT => return CDRF_NOTIFYITEMDRAW as isize,
                    CDDS_ITEMPREPAINT => return CDRF_NOTIFYSUBITEMDRAW as isize,
                    stage if stage == CDDS_ITEMPREPAINT | CDDS_SUBITEM => {
                        if let Some(row) = state
                            .comparison
                            .as_ref()
                            .and_then(|v| v.rows.get(draw.nmcd.dwItemSpec))
                        {
                            if !high_contrast() {
                                let color = match row.change {
                                    EntryChange::Added if draw.iSubItem >= 3 => Some(0x00E3F5E3),
                                    EntryChange::Edited if draw.iSubItem >= 3 => Some(0x00C0EFFF),
                                    EntryChange::Deleted if draw.iSubItem < 3 => Some(0x00E0E0FF),
                                    _ => None,
                                };
                                if let Some(color) = color {
                                    draw.clrTextBk = color;
                                    draw.clrText = 0;
                                }
                            }
                        }
                        return CDRF_NEWFONT as isize;
                    }
                    _ => {}
                }
            }
            if note.code == LVN_ITEMCHANGED {
                if note.idFrom == RECORDS {
                    state.show_record();
                }
                if note.idFrom == ROWS {
                    state.show_entry();
                }
            }
            0
        }
        WM_DPICHANGED => {
            state.dpi = (w & 0xffff) as u32;
            state.set_font();
            let r = &*(l as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            state.layout(hwnd);
            0
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(l as *mut MINMAXINFO);
            m.ptMinTrackSize = POINT {
                x: 800 * state.dpi as i32 / 96,
                y: 600 * state.dpi as i32 / 96,
            };
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            0
        }
        _ => {
            // Default close handling destroys synchronously; release state before reentry.
            drop(state);
            DefWindowProcW(hwnd, message, w, l)
        }
    }
}
unsafe fn high_contrast() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    SystemParametersInfoW(
        SPI_GETHIGHCONTRAST,
        info.cbSize,
        &mut info as *mut _ as *mut _,
        0,
    ) != 0
        && info.dwFlags & HCF_HIGHCONTRASTON != 0
}
/// Open a window with independent bounded storage work; no application drafts move.
pub unsafe fn open(
    parent: HWND,
    directory: PathBuf,
    selected: Option<(Scope, String)>,
) -> Result<HWND, String> {
    if !workers_settled() {
        return Err("Finishing the previous history operation; try again shortly".into());
    }
    let existing = OWNED_WINDOW.get();
    if !existing.is_null() {
        let pointer = GetWindowLongPtrW(existing, GWLP_USERDATA) as *mut RefCell<State>;
        if let Some(cell) = pointer.as_ref() {
            if let Ok(mut state) = cell.try_borrow_mut() {
                if !state.busy {
                    state.selected = selected;
                    let label = state
                        .selected
                        .as_ref()
                        .map(|(s, n)| format!("Selected variable: {} / {}", scope(*s), n))
                        .unwrap_or_else(|| "All variables (no selection)".into());
                    SendMessageW(state.filter, CB_DELETESTRING, 0, 0);
                    SendMessageW(
                        state.filter,
                        CB_INSERTSTRING,
                        0,
                        wide(&label).as_ptr() as isize,
                    );
                    SendMessageW(
                        state.filter,
                        CB_SETCURSEL,
                        if state.selected.is_some() { 0 } else { 1 },
                        0,
                    );
                    state.offset = 0;
                    state.confirming = false;
                    state.request(false);
                }
            }
        }
        ShowWindow(existing, SW_RESTORE);
        SetForegroundWindow(existing);
        return Ok(existing);
    }
    let (sender, requests) = sync_channel(1);
    let (replies, receiver) = sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("history-pages".into())
        .spawn(move || {
            while let Ok(request) = requests.recv() {
                let (result, cleared) = match request {
                    Request::Page(filter, offset) => (
                        storage::records(
                            &directory,
                            false,
                            filter.as_ref().map(|(s, n)| (*s, n.as_str())),
                            offset,
                            50,
                        ),
                        None,
                    ),
                    Request::Clear(filter) => {
                        let selected = filter.as_ref().map(|(s, n)| (*s, n.as_str()));
                        match storage::clear_records(&directory, selected) {
                            Ok(count) => (
                                storage::records(&directory, false, selected, 0, 50),
                                Some(count),
                            ),
                            Err(error) => (Err(error), None),
                        }
                    }
                };
                if replies.send(Reply { result, cleared }).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| format!("Cannot start history worker: {e}"))?;
    let class_name = wide(CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: GetModuleHandleW(null()),
        lpszClassName: class_name.as_ptr(),
        hIcon: crate::icons::application_icon(false),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: (COLOR_BTNFACE + 1) as HBRUSH,
        ..Default::default()
    };
    RegisterClassW(&class);
    let state = Box::new(RefCell::new(State {
        worker: Some(worker),
        sender,
        receiver,
        selected,
        offset: 0,
        busy: false,
        confirming: false,
        records: Vec::new(),
        comparison: None,
        font: null_mut(),
        dpi: 96,
        filter: null_mut(),
        list: null_mut(),
        rows: null_mut(),
        status: null_mut(),
        before: null_mut(),
        after: null_mut(),
        controls: Vec::new(),
    }));
    // If creation fails before WM_NCCREATE, this option still owns the allocation.
    let mut pending = Some(state);
    let hwnd = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        wide(CLASS).as_ptr(),
        wide("Variable history - observed snapshots").as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        1100,
        760,
        parent,
        null_mut(),
        GetModuleHandleW(null()),
        &mut pending as *mut Option<Box<RefCell<State>>> as *const _,
    );
    if hwnd.is_null() {
        return Err("Cannot create history window".into());
    }
    OWNED_WINDOW.set(hwnd);
    Ok(hwnd)
}
/// Release owned history windows and their page allocations before tray idle.
pub unsafe fn close_all() {
    let hwnd = OWNED_WINDOW.get();
    if !hwnd.is_null() {
        DestroyWindow(hwnd);
    }
}
