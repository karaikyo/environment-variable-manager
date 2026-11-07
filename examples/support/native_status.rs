//! Developer diagnostic of the application's visible healthy status.
// The product and reference diagnostics consume different subsets.
#![allow(dead_code)]
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::*,
};
#[derive(Debug, Default)]
pub struct Health {
    pub window_exists: bool,
    pub refresh_enabled: bool,
    pub status_found: bool,
    pub scope_counts: bool,
    pub baseline: bool,
    pub monitoring_error: bool,
    pub status_lines: usize,
    pub exact_healthy_status: bool,
}
impl Health {
    /// Routine refresh may disable controls while the last healthy observation
    /// summary remains valid. Power/read/storage failures replace that summary.
    pub fn observing(&self) -> bool {
        self.window_exists && self.exact_healthy_status
    }
    pub fn healthy(&self) -> bool {
        self.refresh_enabled && self.observing()
    }
}
pub unsafe fn inspect(window: HWND) -> Health {
    unsafe extern "system" fn inspect_child(child: HWND, argument: LPARAM) -> i32 {
        let health = &mut *(argument as *mut Health);
        let mut buffer = [0u16; 2048];
        let length = GetWindowTextW(child, buffer.as_mut_ptr(), buffer.len() as i32);
        let text = String::from_utf16_lossy(&buffer[..length as usize]);
        let count = |text: &str, prefix: &str, suffix: &str| {
            text.strip_prefix(prefix)
                .and_then(|text| text.strip_suffix(suffix))
                .is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
                })
        };
        let mut lines = text.lines();
        let scopes = lines.next().and_then(|line| line.split_once("; "));
        let readable = scopes.is_some_and(|(user, system)| {
            count(user, "User: ", " saved values") && count(system, "System: ", " saved values")
        });
        let baseline = lines.next().is_some_and(|line| {
            count(
                line,
                "Local baseline: ",
                " values. Browsing does not change Windows values.",
            )
        });
        if text.contains("User:")
            || text.contains("Local baseline:")
            || text.contains(" monitoring:")
        {
            health.status_found = true;
            health.scope_counts = readable;
            health.baseline = baseline;
            health.monitoring_error = text.contains(" monitoring:");
            health.status_lines = text.lines().count();
            health.exact_healthy_status = readable && baseline && lines.next().is_none();
            if health.exact_healthy_status {
                return 0;
            }
        }
        1
    }
    let mut result = Health {
        window_exists: IsWindow(window) != 0,
        refresh_enabled: windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(
            GetDlgItem(window, 113),
        ) != 0,
        ..Health::default()
    };
    EnumChildWindows(
        window,
        Some(inspect_child),
        &mut result as *mut Health as LPARAM,
    );
    result
}
pub unsafe fn healthy(window: HWND) -> bool {
    inspect(window).healthy()
}
/// A history HWND alone is not evidence that the page and comparison were rendered.
pub unsafe fn history_loaded(window: HWND) -> bool {
    use windows_sys::Win32::UI::{Controls::*, Input::KeyboardAndMouse::IsWindowEnabled};
    !window.is_null()
        && IsWindowEnabled(GetDlgItem(window, 206)) != 0
        && SendMessageW(GetDlgItem(window, 202), LVM_GETITEMCOUNT, 0, 0) > 0
        && SendMessageW(
            GetDlgItem(window, 202),
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        ) >= 0
        && SendMessageW(GetDlgItem(window, 203), LVM_GETITEMCOUNT, 0, 0) > 0
}

pub unsafe fn find_owned_window(class: &str, process: u32) -> HWND {
    struct Search<'a> {
        class: &'a str,
        process: u32,
        window: HWND,
    }
    unsafe extern "system" fn inspect(window: HWND, argument: LPARAM) -> i32 {
        let search = &mut *(argument as *mut Search<'_>);
        let mut owner = 0;
        GetWindowThreadProcessId(window, &mut owner);
        if owner == search.process {
            let mut name = [0u16; 256];
            let length = GetClassNameW(window, name.as_mut_ptr(), name.len() as i32);
            if String::from_utf16_lossy(&name[..length as usize]) == search.class {
                search.window = window;
                return 0;
            }
        }
        1
    }
    let mut search = Search {
        class,
        process,
        window: std::ptr::null_mut(),
    };
    EnumWindows(Some(inspect), &mut search as *mut Search<'_> as LPARAM);
    search.window
}

/// Sampling health is separate from whether the UI can currently accept a command.
pub unsafe fn observing(window: HWND) -> bool {
    inspect(window).observing()
}
