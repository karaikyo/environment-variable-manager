//! Explicit release metadata checks within the owned lifecycle reference fixture.
use environment_variable_manager::{
    desktop::BrowserOpener,
    native_lifecycle,
    update_check::{CheckControl, HttpResponse, Release, ReleaseTransport, WindowsTransport},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::{Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
};

#[derive(Default)]
pub(super) struct Transport {
    fail: AtomicBool,
}

impl ReleaseTransport for Transport {
    fn fetch(&self, url: &str, control: &CheckControl) -> Result<HttpResponse, String> {
        if self.fail.load(Ordering::Acquire) {
            let end = Instant::now() + Duration::from_millis(500);
            while Instant::now() < end {
                control.check()?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err("Owned fixture connection failure".into())
        } else {
            WindowsTransport.fetch(url, control)
        }
    }
}

pub(super) struct NoBrowser;
impl BrowserOpener for NoBrowser {
    fn open(&self, _: &Release) -> Result<(), String> {
        Err("Reference fixture does not open a browser".into())
    }
}

pub(super) fn check(window: HWND, transport: &Transport, failure: bool) -> Result<String, String> {
    transport.fail.store(failure, Ordering::Release);
    unsafe { SendMessageW(window, native_lifecycle::REOPEN_MESSAGE, 0, 0) };
    super::wait_until("update-check UI ready", || unsafe {
        IsWindowVisible(window) != 0 && super::native_status::healthy(window)
    })?;
    let button = unsafe { GetDlgItem(window, 160) };
    if button.is_null() || unsafe { IsWindowEnabled(button) } == 0 {
        return Err("Update action unavailable".into());
    }
    let started = Instant::now();
    let (mut peak_private, mut peak_working, initial_cpu) = super::counters();
    unsafe { SendMessageW(button, BM_CLICK, 0, 0) };
    loop {
        let (private, working, _) = super::counters();
        peak_private = peak_private.max(private);
        peak_working = peak_working.max(working);
        if unsafe { IsWindowEnabled(button) } != 0 {
            break;
        }
        if started.elapsed() >= Duration::from_secs(20) {
            return Err("Update benchmark check did not finish".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = started.elapsed();
    let cpu = super::counters().2.saturating_sub(initial_cpu);
    let mut buffer = [0u16; 512];
    let length = unsafe {
        GetWindowTextW(
            GetDlgItem(window, 162),
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    let result = String::from_utf16_lossy(&buffer[..length as usize]);
    if failure {
        if !result.contains("Owned fixture connection failure") {
            return Err(format!("Unexpected controlled check result: {result}"));
        }
    } else if !(result.starts_with("Up to date") || result.starts_with("New version available")) {
        return Err(format!(
            "Real GitHub connection check did not succeed: {result}"
        ));
    }
    unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
    super::wait_until("tray after update check", || unsafe {
        IsWindowVisible(window) == 0
    })?;
    Ok(format!(
        "{result:?}, duration_ms={}, sampled_private_peak_bytes={peak_private}, sampled_working_set_peak_bytes={peak_working}, process_cpu_100ns={cpu}",
        elapsed.as_millis()
    ))
}
