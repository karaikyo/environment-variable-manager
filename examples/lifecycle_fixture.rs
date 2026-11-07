//! Disposable native lifecycle/reference-workload fixture. Never touches real Run/environment keys.
use environment_variable_manager::{
    desktop, native_lifecycle,
    startup::{StartupRegistration, WindowsStartup},
    windows_registry::WindowsRegistry,
    Application, RegistryReader, RegistryWriter, Scope, Variable,
};
use std::{
    path::PathBuf,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{ProcessStatus::*, Registry::*, Threading::*},
    UI::{Controls::*, Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
};
#[path = "support/native_status.rs"]
mod native_status;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn reserve(path: &str) {
    let mut key = null_mut();
    let mut disposition = 0;
    assert_eq!(
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            null(),
            &mut key,
            &mut disposition
        ),
        ERROR_SUCCESS
    );
    assert_eq!(
        disposition, REG_CREATED_NEW_KEY,
        "Fixture collision; stop without cleanup"
    );
    RegCloseKey(key);
}
fn reader(root: &str) -> WindowsRegistry {
    WindowsRegistry::under_current_user(&format!("{root}\\User"), &format!("{root}\\System"))
}
fn counters() -> (usize, usize, u64) {
    unsafe {
        let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
        memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        assert_ne!(
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                memory.cb
            ),
            0
        );
        let mut created = FILETIME::default();
        let mut ended = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        assert_ne!(
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut ended,
                &mut kernel,
                &mut user
            ),
            0
        );
        let time = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        (
            memory.PrivateUsage,
            memory.WorkingSetSize,
            time(kernel) + time(user),
        )
    }
}
fn wait_until(label: &str, mut condition: impl FnMut() -> bool) -> Result<(), String> {
    println!("Waiting: {label}");
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        if Instant::now() >= deadline {
            return Err(format!("Native fixture timed out: {label}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
fn main() {
    assert!(
        unsafe { native_status::find_owned_window("EnvironmentManager.Main", std::process::id()) }
            .is_null(),
        "Exit any existing app before this fixture"
    );
    let args = std::env::args().collect::<Vec<_>>();
    let directory = PathBuf::from(args.get(1).expect("Use a fresh disposable directory"));
    let release_benchmark = args.iter().any(|a| a == "--release-benchmark");
    let long_run = args.iter().any(|a| a == "--long-run");
    let diagnose_status = args.iter().any(|a| a == "--diagnose-status");
    let diagnose_refresh = args.iter().any(|a| a == "--diagnose-refresh");
    assert!(
        !diagnose_refresh || diagnose_status,
        "Refresh injection is diagnostic-only"
    );
    let draft_smoke = args.iter().any(|a| a == "--draft-smoke");
    let resume = args
        .iter()
        .any(|a| a == "--resume" || a == "--resume-smoke");
    let smoke = draft_smoke || args.iter().any(|a| a == "--smoke" || a == "--resume-smoke");
    assert!(!directory.exists());
    std::fs::create_dir(&directory).unwrap();
    if diagnose_status {
        std::fs::write(
            directory.join("DIAGNOSTIC-ONLY.txt"),
            "Not resource acceptance evidence: two-second settle and ten-millisecond samples.\n",
        )
        .unwrap();
    }
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = format!("Software\\EnvironmentManagerTests\\lifecycle-{suffix}");
    unsafe {
        reserve(&root);
        reserve(&format!("{root}\\User"));
        reserve(&format!("{root}\\System"));
        reserve(&format!("{root}\\Run"));
    }
    let registry = reader(&root);
    let mut expected = Vec::new();
    for i in 0..200 {
        let entries = (0..10)
            .map(|e| format!("owned-{i}-{e}"))
            .collect::<Vec<_>>()
            .join(";");
        let variable = Variable::text(Scope::User, &format!("Fixture{i:03}"), &entries, false);
        registry.write_user(&variable).unwrap();
        expected.push(variable);
    }
    let data = directory.join("data");
    let query = Application::open(&registry, &data);
    assert!(query.storage_error().is_none());
    // Benchmark setup only; behavioral assertions below use Application queries.
    {
        let mut db = rusqlite::Connection::open(data.join("environment.sqlite")).unwrap();
        let tx = db.transaction().unwrap();
        {
            let mut insert=tx.prepare("INSERT INTO operations(scope,name,before_type,before_raw,desired_type,desired_raw,after_type,after_raw,time_ms,source,outcome) VALUES(0,?1,1,?2,1,?2,1,?2,0,'fixture','saved')").unwrap();
            for i in 0..if resume { 9998 } else { 9999 } {
                let value = &expected[i % 200];
                insert
                    .execute(rusqlite::params![value.name, value.bytes])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    assert_eq!(
        query
            .history(None, if resume { 9997 } else { 9998 }, 10)
            .unwrap()
            .len(),
        1
    );
    let registration =
        WindowsStartup::under_current_user(&format!("{root}\\Run"), "OwnedLifecycleFixture");
    assert!(registration.state().unwrap().command.is_none());
    std::fs::write(directory.join("fixture.txt"),format!("root={root}\nvariables=200\nentries=2000\nhistory=10000\nprocesses=1\nlogical_cpus={}\n",std::thread::available_parallelism().unwrap())).unwrap();
    let output = directory.clone();
    let thread_root = root.clone();
    let exercise = std::thread::spawn(move || {
        let mut main = null_mut();
        let result = (|| -> Result<(), String> {
            wait_until("main ready", || {
                main = unsafe {
                    native_status::find_owned_window("EnvironmentManager.Main", std::process::id())
                };
                if main.is_null() {
                    return false;
                }
                let mut text = [0u16; 256];
                let len = unsafe { GetWindowTextW(main, text.as_mut_ptr(), 256) };
                String::from_utf16_lossy(&text[..len as usize]) == "Environment Manager"
                    && unsafe { native_status::healthy(main) }
            })?;
            unsafe {
                PostMessageW(main, WM_CLOSE, 0, 0);
            }
            wait_until("main hidden", || unsafe { IsWindowVisible(main) == 0 })?;
            let external = reader(&thread_root);
            let changed=Variable::text(Scope::User,"Fixture000","changed externally while in tray;owned-0-1;owned-0-2;owned-0-3;owned-0-4;owned-0-5;owned-0-6;owned-0-7;owned-0-8;owned-0-9",false);
            external.write_user(&changed).unwrap();
            wait_until("background history", || {
                query
                    .history(Some((Scope::User, "Fixture000")), 0, 100)
                    .unwrap()
                    .iter()
                    .any(|h| {
                        h.after.as_ref() == Some(&changed) && h.source == "external (actor unknown)"
                    })
            })?;
            let ready_counters = counters();
            let mut csv =
                String::from("phase,sample,private_bytes,working_set_bytes,process_cpu_100ns\n");
            let mut summary =
                String::from("native_message_smoke=passed\nbackground_recording=passed\n");
            summary.push_str(&format!(
                "first_tray_private_bytes={}\nfirst_tray_working_set_bytes={}\n",
                ready_counters.0, ready_counters.1
            ));
            if resume {
                for cycle in 0..2 {
                    unsafe {
                        SendMessageW(main, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    let value = Variable::text(Scope::User,"Fixture001","net change during simulated suspend;owned-1-1;owned-1-2;owned-1-3;owned-1-4;owned-1-5;owned-1-6;owned-1-7;owned-1-8;owned-1-9",false);
                    if cycle == 1 {
                        external.write_user(&value).unwrap();
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    assert_eq!(
                        query.history(None, 9998, 10).unwrap().len(),
                        1,
                        "Suspended observation preserves baseline"
                    );
                    unsafe {
                        SendMessageW(main, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize, 0);
                        SendMessageW(main, WM_POWERBROADCAST, PBT_APMRESUMESUSPEND as usize, 0);
                        SendMessageW(main, WM_COMMAND, 113, 0);
                    }
                    if cycle == 1 {
                        wait_until("native resume gap recorded", || {
                            query
                                .history(Some((Scope::User, "Fixture001")), 0, 100)
                                .unwrap()
                                .iter()
                                .any(|h| h.outcome == "gap-net" && h.after.as_ref() == Some(&value))
                        })?;
                    } else {
                        wait_until("unchanged resume completed", || unsafe {
                            native_status::healthy(main)
                        })?;
                        assert_eq!(
                            query.history(None, 9998, 10).unwrap().len(),
                            1,
                            "Unchanged resume fabricates no change"
                        );
                    }
                }
                wait_until(
                    "changed resume completed with healthy observation",
                    || unsafe { native_status::healthy(main) },
                )?;
                assert_eq!(
                    query.history(None, 9998, 10).unwrap().len(),
                    2,
                    "Overlapping power/Refresh/watch signals deduplicate"
                );
                summary.push_str("native_simulated_power_messages=passed\nactual_sleep_hibernate=not_performed\n");
            }
            let mut phases = vec![
                if resume {
                    "after_simulated_resume"
                } else {
                    "initial"
                },
                "after_history",
            ];
            if release_benchmark {
                phases.push("after_simulated_resume");
            }
            if long_run {
                phases.push("longer_run");
            }
            for phase in phases {
                if release_benchmark && phase == "after_simulated_resume" {
                    unsafe {
                        SendMessageW(main, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
                    }
                    wait_until("suspend interruption exposed", || unsafe {
                        !native_status::healthy(main)
                    })?;
                    unsafe {
                        SendMessageW(main, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize, 0);
                        SendMessageW(main, WM_POWERBROADCAST, PBT_APMRESUMESUSPEND as usize, 0);
                    }
                    wait_until("completed healthy resume reconciliation", || unsafe {
                        native_status::healthy(main)
                    })?;
                    assert_eq!(
                        query.history(None, 9999, 10).unwrap().len(),
                        1,
                        "Unchanged native resume retains exactly 10000 records"
                    );
                    summary.push_str("same_process_unchanged_resume=passed\nactual_sleep_hibernate=not_performed\n");
                }
                if phase == "after_history" {
                    unsafe {
                        PostMessageW(main, native_lifecycle::REOPEN_MESSAGE, 0, 0);
                    }
                    wait_until("main reopened", || unsafe {
                        IsWindowVisible(main) != 0 && IsWindowEnabled(GetDlgItem(main, 112)) != 0
                    })?;
                    unsafe {
                        PostMessageW(main, WM_COMMAND, 112, 0);
                    }
                    wait_until("history page and selected comparison loaded", || unsafe {
                        native_status::history_loaded(native_status::find_owned_window(
                            "EnvironmentManager.PagedHistory",
                            std::process::id(),
                        ))
                    })?;
                    let opened = counters();
                    summary.push_str(&format!(
                        "history_open_private_bytes={}\nhistory_open_working_set_bytes={}\n",
                        opened.0, opened.1
                    ));
                    unsafe {
                        PostMessageW(main, WM_CLOSE, 0, 0);
                    }
                    wait_until("history released and main hidden",||unsafe{IsWindowVisible(main)==0} && unsafe{native_status::find_owned_window("EnvironmentManager.PagedHistory", std::process::id())}.is_null())?;
                }
                if !smoke {
                    let samples = if diagnose_status {
                        200
                    } else if phase == "longer_run" {
                        180
                    } else {
                        60
                    };
                    let settle = if diagnose_status { 2 } else { 30 };
                    let interval_ms = if diagnose_status { 10 } else { 1000 };
                    println!("{phase}: settling {settle}s, then {samples} samples every {interval_ms}ms; diagnostic_only={diagnose_status}");
                    std::thread::sleep(Duration::from_secs(settle));
                    let started = Instant::now();
                    let initial = counters().2;
                    for sample in 1..=samples {
                        std::thread::sleep(Duration::from_millis(interval_ms));
                        if diagnose_refresh && phase == "after_history" {
                            unsafe {
                                SendMessageW(main, WM_COMMAND, 113, 0);
                            }
                        }
                        let health = unsafe { native_status::inspect(main) };
                        if !health.observing() {
                            std::fs::write(output.join("samples.csv"), &csv).unwrap();
                            let failure = format!("phase={phase}; sample={sample}; diagnostic_only={diagnose_status}; {health:?}");
                            std::fs::write(output.join("failure.txt"), &failure).unwrap();
                            return Err(failure);
                        }
                        let (private, working, cpu) = counters();
                        csv.push_str(&format!("{phase},{sample},{private},{working},{cpu}\n"));
                    }
                    let cpu = (counters().2 - initial) as f64
                        / 10_000_000.0
                        / started.elapsed().as_secs_f64()
                        / std::thread::available_parallelism().unwrap().get() as f64
                        * 100.0;
                    summary.push_str(&format!("{phase}_cpu_percent={cpu:.6}\n"));
                    std::fs::write(output.join("samples.csv"), &csv).unwrap();
                }
            }
            if draft_smoke {
                unsafe {
                    SendMessageW(main, native_lifecycle::REOPEN_MESSAGE, 0, 0);
                }
                wait_until("new draft control ready", || unsafe {
                    IsWindowVisible(main) != 0 && IsWindowEnabled(GetDlgItem(main, 131)) != 0
                })?;
                unsafe {
                    SetWindowTextW(GetDlgItem(main, 130), wide("OwnedUnsavedDraft").as_ptr());
                    PostMessageW(main, WM_COMMAND, 131, 0);
                }
                std::thread::sleep(Duration::from_millis(150));
                // Select an existing variable so only the unselected creation is dirty.
                let item = LVITEMW {
                    stateMask: LVIS_SELECTED | LVIS_FOCUSED,
                    state: LVIS_SELECTED | LVIS_FOCUSED,
                    ..Default::default()
                };
                unsafe {
                    SendMessageW(
                        GetDlgItem(main, 103),
                        LVM_SETITEMSTATE,
                        0,
                        &item as *const _ as isize,
                    );
                    PostMessageW(main, WM_CLOSE, 0, 0);
                }
                wait_until("draft tray", || unsafe { IsWindowVisible(main) == 0 })?;
                unsafe {
                    PostMessageW(main, WM_COMMAND, native_lifecycle::EXIT, 0);
                }
                wait_until("inline unsaved exit choices", || unsafe {
                    IsWindowVisible(GetDlgItem(main, 153)) != 0
                        && IsWindowVisible(GetDlgItem(main, 154)) != 0
                })?;
                for (id, expected) in [
                    (153, "Exit and discard unsaved edits"),
                    (154, "Cancel exit"),
                ] {
                    let mut text = [0u16; 128];
                    let len =
                        unsafe { GetWindowTextW(GetDlgItem(main, id), text.as_mut_ptr(), 128) };
                    if String::from_utf16_lossy(&text[..len as usize]) != expected {
                        return Err("Unexpected exit choice label".into());
                    }
                }
                unsafe {
                    PostMessageW(main, WM_COMMAND, 154, 0);
                }
                wait_until("cancel exit keeps app", || unsafe {
                    IsWindowVisible(main) != 0 && IsWindowVisible(GetDlgItem(main, 153)) == 0
                })?;
                unsafe {
                    PostMessageW(main, WM_COMMAND, native_lifecycle::EXIT, 0);
                }
                wait_until("draft retained after cancel", || unsafe {
                    IsWindowVisible(GetDlgItem(main, 153)) != 0
                })?;
                unsafe {
                    PostMessageW(main, WM_COMMAND, 153, 0);
                }
                summary.push_str("native_unselected_draft_exit_cancel_discard=passed\n");
            }
            unsafe {
                let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
                memory.cb = std::mem::size_of_val(&memory) as u32;
                assert_ne!(
                    GetProcessMemoryInfo(
                        GetCurrentProcess(),
                        &mut memory as *mut _ as _,
                        memory.cb
                    ),
                    0
                );
                summary.push_str(&format!(
                    "whole_fixture_peak_working_set_bytes={}\nwhole_fixture_peak_commit_bytes={}\n",
                    memory.PeakWorkingSetSize, memory.PeakPagefileUsage
                ));
            }
            std::fs::write(output.join("result.txt"), summary).unwrap();
            Ok(())
        })();
        if !main.is_null() {
            unsafe {
                PostMessageW(main, WM_COMMAND, native_lifecycle::EXIT, 0);
            }
        }
        result
    });
    let outcome = desktop::run_with_startup(
        registry,
        data,
        registration,
        std::env::current_exe().unwrap(),
        false,
    );
    let exercise_result = exercise.join().unwrap();
    outcome.unwrap();
    let registration =
        WindowsStartup::under_current_user(&format!("{root}\\Run"), "OwnedLifecycleFixture");
    let command = registration.state().unwrap().command;
    assert!(command.is_some());
    registration.replace(command.as_deref(), None).unwrap();
    assert!(registration.state().unwrap().command.is_none());
    let registry = reader(&root);
    expected[0]=Variable::text(Scope::User,"Fixture000","changed externally while in tray;owned-0-1;owned-0-2;owned-0-3;owned-0-4;owned-0-5;owned-0-6;owned-0-7;owned-0-8;owned-0-9",false);
    if resume {
        expected[1]=Variable::text(Scope::User,"Fixture001","net change during simulated suspend;owned-1-1;owned-1-2;owned-1-3;owned-1-4;owned-1-5;owned-1-6;owned-1-7;owned-1-8;owned-1-9",false);
    }
    let values = registry.read_scope(Scope::User).unwrap();
    assert_eq!(values.len(), 200);
    for value in &expected {
        assert!(values.contains(value));
    }
    assert!(registry.read_scope(Scope::System).unwrap().is_empty());
    // Verify the entire owned subtree shape before removing only that exclusive subtree.
    unsafe {
        let mut key = null_mut();
        assert_eq!(
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(&root).as_ptr(),
                0,
                KEY_READ,
                &mut key
            ),
            ERROR_SUCCESS
        );
        let mut subkeys = 0;
        let mut values = 0;
        assert_eq!(
            RegQueryInfoKeyW(
                key,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut subkeys,
                null_mut(),
                null_mut(),
                &mut values,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut()
            ),
            ERROR_SUCCESS
        );
        assert_eq!((subkeys, values), (3, 0));
        RegCloseKey(key);
        assert_eq!(
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(&root).as_ptr()),
            ERROR_SUCCESS
        );
    }
    assert!(
        exercise_result.is_ok(),
        "Native exercise failed; owned registry cleaned: {exercise_result:?}"
    );
    println!(
        "Fixture passed; owned registry cleaned; evidence: {}",
        directory.display()
    );
}
