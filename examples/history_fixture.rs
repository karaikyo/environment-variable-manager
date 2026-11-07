#![windows_subsystem = "windows"]
//! Developer-only lifecycle/memory smoke; never included in the portable product.
//! Uses disposable in-memory Windows values and the production history window.
use environment_variable_manager::{
    history_window, same_name, Application, RegistryReader, RegistryWriter, Scope, Variable,
};
use std::{
    cell::RefCell,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{
        LibraryLoader::GetModuleHandleW,
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    },
    UI::{Controls::*, HiDpi::*, WindowsAndMessaging::*},
};
struct Registry(RefCell<Vec<Variable>>);
impl RegistryReader for Registry {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(self
            .0
            .borrow()
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
impl RegistryWriter for Registry {
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        *self
            .0
            .borrow_mut()
            .iter_mut()
            .find(|v| v.scope == value.scope && same_name(&v.name, &value.name))
            .unwrap() = value.clone();
        Ok(())
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe extern "system" fn proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, message, w, l)
}
unsafe fn memory(phase: &str) -> String {
    let mut memory = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    assert_ne!(
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut memory as *mut _ as *mut _,
            memory.cb
        ),
        0
    );
    format!(
        "{{\"phase\":\"{phase}\",\"private_bytes\":{},\"working_set_bytes\":{}}}",
        memory.PrivateUsage, memory.WorkingSetSize
    )
}
fn main() {
    let directory = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("history-fixture-data");
    assert!(!directory.exists(), "Fixture directory must be new");
    let registry = Registry(RefCell::new(
        (0..200)
            .map(|i| {
                Variable::text(
                    if i < 100 { Scope::User } else { Scope::System },
                    &format!("Fixture{i}"),
                    " a;;duplicate;duplicate;%ROOT%;entry;entry;entry;entry;",
                    false,
                )
            })
            .collect(),
    ));
    let mut app = Application::open(&registry, &directory);
    for i in 0..105 {
        app.edit_entries(
            Scope::User,
            "Fixture0",
            vec![
                format!("edit{i}"),
                "".into(),
                "duplicate".into(),
                "duplicate".into(),
                "%ROOT%".into(),
                "entry".into(),
                "entry".into(),
                "entry".into(),
                "entry".into(),
                "".into(),
            ],
        )
        .unwrap();
        app.save(&registry, Scope::User, "Fixture0").unwrap();
    }
    assert_eq!(app.history(None, 100, 100).unwrap().len(), 5);
    drop(app);
    drop(registry);
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&controls);
        let name = wide("EnvironmentManager.HistoryFixture");
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE + 1) as HBRUSH,
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("History lifecycle fixture - no Windows writes").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            650,
            240,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        assert!(!hwnd.is_null());
        SetTimer(hwnd, 1, 100, None);
        let start = Instant::now();
        let mut stage = 0;
        let mut evidence = Vec::new();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_TIMER && msg.hwnd == hwnd {
                let elapsed = start.elapsed();
                if stage == 0 && elapsed >= Duration::from_secs(10) {
                    evidence.push(memory("before_history"));
                    let first = history_window::open(
                        hwnd,
                        directory.clone(),
                        Some((Scope::User, "Fixture0".into())),
                    )
                    .unwrap();
                    let reused = history_window::open(hwnd, directory.clone(), None).unwrap();
                    assert_eq!(first, reused, "Repeated open must reuse window/worker");
                    stage = 1;
                }
                if stage == 1 && elapsed >= Duration::from_secs(15) {
                    evidence.push(memory("history_open"));
                    history_window::close_all();
                    stage = 2;
                }
                if stage == 2 && elapsed >= Duration::from_secs(25) {
                    evidence.push(memory("after_history_closed"));
                    std::fs::write(directory.join("memory.json"),format!("{{\"variables\":200,\"entries\":2000,\"history_records\":105,\"page_records\":50,\"settle_seconds\":10,\"samples\":[{}]}}",evidence.join(","))).unwrap();
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    stage = 3;
                }
            }
            if IsDialogMessageW(GetAncestor(msg.hwnd, GA_ROOT), &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        assert_eq!(stage, 3, "Fixture closed before lifecycle smoke completed");
    }
}
