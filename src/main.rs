#![cfg_attr(not(test), windows_subsystem = "windows")]
use environment_variable_manager::{desktop, windows_registry::WindowsRegistry};
fn main() {
    if let Some(code) = environment_variable_manager::elevation::dispatch_helper() {
        std::process::exit(code);
    }
    if environment_variable_manager::elevation::require_unelevated().is_err() {
        std::process::exit(73);
    }
    let _instance = match environment_variable_manager::native_lifecycle::Instance::product() {
        Ok(Some(instance)) => instance,
        Ok(None) => return,
        Err(_) => std::process::exit(1),
    };
    let result = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|path| {
            let parent = path.parent().ok_or("Application directory unavailable")?;
            desktop::run(WindowsRegistry::environment(), parent.join("data"))
        });
    if let Err(error) = result {
        eprintln!("Startup failed: {error}");
        std::process::exit(1);
    }
}
