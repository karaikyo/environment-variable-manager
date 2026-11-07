use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/icon.ico");
    println!("cargo:rerun-if-env-changed=EVM_RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let compiler = env::var_os("EVM_RC").map(PathBuf::from).unwrap_or_else(|| {
        let base = PathBuf::from(
            env::var_os("ProgramFiles(x86)").expect("Windows SDK location unavailable"),
        )
        .join("Windows Kits/10/bin");
        let mut candidates: Vec<_> = std::fs::read_dir(base)
            .expect("Installed Windows SDK required")
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("x64/rc.exe"))
            .filter(|path| path.is_file())
            .collect();
        candidates.sort();
        candidates
            .pop()
            .expect("Installed Windows SDK resource compiler required; set EVM_RC to its path")
    });
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("application.res");
    let mut command = Command::new(compiler);
    command
        .current_dir("res")
        .arg("/nologo")
        .arg("/fo")
        .arg(&output)
        .arg("app.rc");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    assert!(
        command
            .status()
            .expect("Cannot run installed resource compiler")
            .success(),
        "Icon resource compilation failed"
    );
    println!("cargo:rustc-link-arg={}", output.display());
}
