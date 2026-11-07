# Environment Manager

Portable Windows 11 x64 environment-variable manager built with Rust, native
Windows controls and bundled SQLite. Version 0.1.2 includes the application icon
and corrected path previews. This is a preview release; controlled native acceptance
checks remain open.

Extract the package into a writable local folder and run
`environment-variable-manager.exe` as an ordinary user. Sign-in startup is enabled
by default after healthy initialization; the main window lets you disable it.
Closing the window keeps monitoring in the tray. Use **Exit** to stop it.
[Run and update instructions](RUN-UPDATE.md) explain data preservation and backups.

Browse and search user/system variables separately, with each semicolon-delimited
value entry on its own row. Save applies the selected draft directly; Discard
resets only that draft. Navigation and the tray preserve unsaved drafts. Creating,
deleting and preserving plain/expandable value types are supported. System writes
request same-account administrator consent through a short-lived helper.

History records app-made edits and observed persistent external changes. It is an
observed history, not a complete audit. The separate history window shows exact
before/after entries, marks additions/edits/deletions with color and text, and
supports selected/all-variable browsing and clearing completed history. Settings,
baselines, history and pending recovery live unencrypted in `data/` beside the
executable. Storage failure blocks app writes and preserves data for recovery.

The implementation includes tray/startup/exit, monitoring recovery after resume,
and verified-backup database migration. Actual sleep/hibernate, sign-in/Explorer,
UAC system writes, Windows environment UI edits, visual/keyboard/accessibility,
cold-start timing and physical-disk-full checks still require the
controlled acceptance session. Automated
fixtures do not turn these unperformed checks into a release pass.

## Offline development

Use the installed Rust/MSVC/Windows SDK toolchain and Cargo cache. Build scripts do
not download or install software. Local tool paths are explicit in
`scripts/cargo.ps1`, which configures only its child process.

```powershell
./scripts/build.ps1                  # release product
./scripts/test.ps1                   # behavioral and owned native fixtures
./scripts/build.ps1 -Fixtures        # developer diagnostics, never packaged
./scripts/test-package.ps1           # build and verify clean archive contents
./scripts/package.ps1                # versioned x64 candidate in dist/
```

Packaging uses an explicit file whitelist and refuses to overwrite an existing
archive. Choose a new `-OutputDirectory` for another build. Keep `data/` when
updating, and never share it as part of an application package. All test fixtures
use disposable data and owned registry keys/variables. Product execution copies
may use ordinary-user Temp folders to avoid this workspace's low-integrity label;
no security policy is changed. Required [third-party notices](THIRD-PARTY-NOTICES.md)
are included in the archive. There is no application runtime installation,
telemetry, network dependency, automatic updater or restore-from-history action.
