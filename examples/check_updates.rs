//! Explicit metadata-only connection diagnostic; never runs as part of tests.
use environment_variable_manager::update_check::{UpdateChecker, UpdateStatus, WindowsTransport};
use std::{sync::Arc, time::Duration};
fn main() {
    if std::env::args().nth(1).as_deref() != Some("--check") {
        eprintln!("Pass --check to explicitly query public GitHub release metadata. No package is downloaded.");
        return;
    }
    let mut checker = UpdateChecker::new(Arc::new(WindowsTransport));
    checker.start();
    while checker.is_running() {
        checker.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("{:?}", checker.status());
    if matches!(checker.status(), UpdateStatus::Failed(_)) {
        std::process::exit(1);
    }
}
