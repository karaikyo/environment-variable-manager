//! Disposable manual-update diagnostic. Reads real Windows values; never writes them.
use environment_variable_manager::{
    startup::{RegistrationState, Startup, StartupRegistration},
    windows_registry::WindowsRegistry,
    Application, RegistryReader, Scope, Variable,
};
use std::path::Path;
#[path = "support/native_status.rs"]
mod native_status;
struct Snapshot(Vec<Variable>);
impl RegistryReader for Snapshot {
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        Ok(self
            .0
            .iter()
            .filter(|v| v.scope == scope)
            .cloned()
            .collect())
    }
}
struct Unreadable;
impl RegistryReader for Unreadable {
    fn read_scope(&self, _: Scope) -> Result<Vec<Variable>, String> {
        Err("Read persisted data without observing Windows".into())
    }
}
struct Blocked;
impl StartupRegistration for Blocked {
    fn state(&self) -> Result<RegistrationState, String> {
        Ok(RegistrationState {
            command: None,
            blocked: true,
        })
    }
    fn replace(&self, _: Option<&str>, _: Option<&str>) -> Result<(), String> {
        panic!("seed must never register startup")
    }
}
fn all(app: &Application) -> Vec<environment_variable_manager::HistoryRecord> {
    let mut rows = vec![];
    loop {
        let page = app.history(None, rows.len(), 100).unwrap();
        if page.is_empty() {
            return rows;
        }
        rows.extend(page);
    }
}

fn launch(executable: &Path, metrics: Option<(&Path, bool)>) {
    use std::{
        os::windows::io::AsRawHandle,
        ptr::null_mut,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        System::Threading::WaitForInputIdle,
        UI::WindowsAndMessaging::*,
    };
    struct Search {
        process: u32,
        window: HWND,
    }
    unsafe extern "system" fn find(window: HWND, argument: LPARAM) -> i32 {
        let search = &mut *(argument as *mut Search);
        let mut owner = 0;
        GetWindowThreadProcessId(window, &mut owner);
        if owner != search.process {
            return 1;
        }
        let mut class = [0u16; 256];
        let length = GetClassNameW(window, class.as_mut_ptr(), 256);
        if String::from_utf16_lossy(&class[..length as usize]) != "EnvironmentManager.Main" {
            return 1;
        }
        let mut title = [0u16; 256];
        let length = GetWindowTextW(window, title.as_mut_ptr(), 256);
        if String::from_utf16_lossy(&title[..length as usize]) != "Environment Manager" {
            return 1;
        }
        search.window = window;
        0
    }
    let started = Instant::now();
    let mut child = std::process::Command::new(executable)
        .current_dir(executable.parent().unwrap())
        .spawn()
        .unwrap();
    let result = (|| -> Result<(), String> {
        if unsafe { WaitForInputIdle(child.as_raw_handle() as _, 10_000) } != 0 {
            return Err("Product did not become input idle".into());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let window = loop {
            if child.try_wait().unwrap().is_some() {
                return Err("Product exited before readiness".into());
            }
            let mut search = Search {
                process: child.id(),
                window: null_mut(),
            };
            unsafe {
                EnumWindows(Some(find), &mut search as *mut _ as LPARAM);
            }
            if !search.window.is_null()
                && (metrics.is_none() || unsafe { native_status::healthy(search.window) })
            {
                break search.window;
            }
            if Instant::now() >= deadline {
                return Err("Product did not become browse ready".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let ready_ms = started.elapsed().as_secs_f64() * 1000.0;
        if let Some((directory, sample)) = metrics {
            std::fs::create_dir(directory).unwrap();
            let counters = || {
                use windows_sys::Win32::System::ProcessStatus::*;
                let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
                memory.cb = std::mem::size_of_val(&memory) as u32;
                assert_ne!(
                    unsafe {
                        GetProcessMemoryInfo(
                            child.as_raw_handle() as _,
                            &mut memory as *mut _ as _,
                            memory.cb,
                        )
                    },
                    0
                );
                memory
            };
            let cpu_time = || {
                use windows_sys::Win32::{
                    Foundation::FILETIME, System::Threading::GetProcessTimes,
                };
                let mut created = FILETIME::default();
                let mut ended = FILETIME::default();
                let mut kernel = FILETIME::default();
                let mut user = FILETIME::default();
                assert_ne!(
                    unsafe {
                        GetProcessTimes(
                            child.as_raw_handle() as _,
                            &mut created,
                            &mut ended,
                            &mut kernel,
                            &mut user,
                        )
                    },
                    0
                );
                let ticks =
                    |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
                ticks(kernel) + ticks(user)
            };
            let ready = counters();
            let mut report = format!("ready_milliseconds={ready_ms:.3}\nready_private_bytes={}\nready_working_set_bytes={}\nready_peak_working_set_bytes={}\nready_peak_commit_bytes={}\n",ready.PrivateUsage,ready.WorkingSetSize,ready.PeakWorkingSetSize,ready.PeakPagefileUsage);
            if sample {
                unsafe {
                    PostMessageW(window, WM_CLOSE, 0, 0);
                }
                std::thread::sleep(Duration::from_secs(30));
                assert_eq!(unsafe { IsWindowVisible(window) }, 0);
                let mut csv=String::from("sample,private_bytes,working_set_bytes,peak_working_set_bytes,peak_commit_bytes\n");
                let sample_started = Instant::now();
                let initial_cpu = cpu_time();
                for index in 1..=60 {
                    std::thread::sleep(Duration::from_secs(1));
                    let health = unsafe { native_status::inspect(window) };
                    if !health.observing() {
                        std::fs::write(directory.join("samples.csv"), &csv).unwrap();
                        let failure = format!("phase=product_tray; sample={index}; {health:?}");
                        std::fs::write(directory.join("failure.txt"), &failure).unwrap();
                        return Err(failure);
                    }
                    let memory = counters();
                    csv.push_str(&format!(
                        "{index},{},{},{},{}\n",
                        memory.PrivateUsage,
                        memory.WorkingSetSize,
                        memory.PeakWorkingSetSize,
                        memory.PeakPagefileUsage
                    ));
                }
                let cpu_percent = (cpu_time() - initial_cpu) as f64
                    / 10_000_000.0
                    / sample_started.elapsed().as_secs_f64()
                    / std::thread::available_parallelism().unwrap().get() as f64
                    * 100.0;
                report.push_str(&format!("idle_cpu_percent={cpu_percent:.6}\n"));
                std::fs::write(directory.join("samples.csv"), csv).unwrap();
                report.push_str("tray_samples=60\nsettle_seconds=30\nprocesses=1\n");
            }
            std::fs::write(directory.join("result.txt"), report).unwrap();
        }
        unsafe {
            PostMessageW(
                window,
                WM_COMMAND,
                environment_variable_manager::native_lifecycle::EXIT,
                0,
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("Product failed: {status}"))
                };
            }
            if Instant::now() >= deadline {
                return Err("Product did not finish explicit Exit".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if result.is_err() && child.try_wait().unwrap().is_none() {
        child
            .kill()
            .expect("stop only the owned failed child fixture");
        child.wait().unwrap();
    }
    result.unwrap();
    println!("Owned product clean native startup and explicit Exit");
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let command = args[1].to_str().unwrap();
    let dir = Path::new(&args[2]);
    if command == "launch" {
        launch(dir, None);
    } else if command == "launch-timing" || command == "launch-sample" {
        launch(dir, Some((Path::new(&args[3]), command == "launch-sample")));
    } else if command == "seed" || command == "seed-clean" {
        assert!(!dir.exists(), "seed only a new owned data directory");
        let registry = WindowsRegistry::environment();
        let mut values = registry.read_scope(Scope::User).unwrap();
        values.extend(registry.read_scope(Scope::System).unwrap());
        let mut app = Application::open(&Snapshot(values.clone()), dir);
        assert_eq!(app.storage_error(), None);
        let startup = Startup::initialize(&app, &Blocked, Path::new("C:/Owned/app.exe")).unwrap();
        assert!(!startup.preferred_enabled());
        if command == "seed-clean" {
            println!(
                "Clean disposable data: {} baseline values; {} entries; 0 completed history records; startup disabled; no Windows writes",
                values.len(), values.iter().filter_map(Variable::entries).map(|entries| entries.len()).sum::<usize>()
            );
            return;
        }
        let mut changed = values.clone();
        let name = format!("EVM_UPDATE_OWNED_{}", std::process::id());
        assert!(!values.iter().any(|v| v.name == name));
        changed.push(Variable::text(Scope::User, &name, " %ROOT%;a;;a;\n ", true));
        changed.push(Variable {
            scope: Scope::System,
            name: format!("{name}_RAW"),
            registry_type: 7,
            bytes: vec![0, 255, 42],
        });
        app.observe(&Snapshot(changed));
        app.observe(&Snapshot(values));
        assert_eq!(all(&app).len(), 4);
        drop(app);
        // Older-release fixture generation only, never a product downgrade path.
        let db = rusqlite::Connection::open(dir.join("environment.sqlite")).unwrap();
        db.execute("INSERT INTO operations(scope,name,desired_type,desired_raw,time_ms,source,outcome) VALUES(0,?1,1,?2,1235,'app-made','pending')",rusqlite::params![name,Variable::text(Scope::User,"unused","desired",false).bytes]).unwrap();
        db.execute_batch("PRAGMA user_version=1").unwrap();
        println!("Seeded owned schema1 data: 4 completed history records and 1 pending intent; startup disabled; no Windows writes");
    } else if command == "compare" {
        let reference = Path::new(&args[3]);
        let old = Application::open(&Unreadable, reference);
        let updated = Application::open(&Unreadable, dir);
        assert_eq!(old.baseline(), updated.baseline());
        assert_eq!(all(&old), all(&updated));
        assert_eq!(
            old.pending_operations().unwrap(),
            updated.pending_operations().unwrap()
        );
        assert_eq!(old.pending_operations().unwrap().len(), 1);
        assert!(updated.storage_error().unwrap().contains("Pending"));
        // Check the saved setting through Startup on disposable copies after
        // explicit pending reconciliation, leaving the update evidence intact.
        for source in [reference, dir] {
            let copy = std::env::temp_dir().join(format!(
                "evm-update-setting-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&copy).unwrap();
            std::fs::copy(
                source.join("environment.sqlite"),
                copy.join("environment.sqlite"),
            )
            .unwrap();
            let mut app = Application::open(&Unreadable, &copy);
            let snapshot = Snapshot(app.baseline().unwrap().to_vec());
            app.refresh(&snapshot);
            assert_eq!(app.storage_error(), None);
            let startup =
                Startup::initialize(&app, &Blocked, Path::new("C:/Owned/app.exe")).unwrap();
            assert!(!startup.preferred_enabled() && !startup.enabled());
            drop((app, startup));
            std::fs::remove_dir_all(&copy).unwrap();
        }
        println!("Preserved exactly: {} baseline variables, {} completed history records, {} pending intent",updated.baseline().unwrap().len(),all(&updated).len(),updated.pending_operations().unwrap().len());
    } else {
        panic!("expected seed or compare");
    }
}
