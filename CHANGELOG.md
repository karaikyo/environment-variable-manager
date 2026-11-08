# Changelog

Notable changes to Environment Manager are recorded here, newest first.
Versions 0.1.0 and 0.1.1 were local candidates. Version 0.1.2 is a GitHub preview
release; controlled native acceptance checks remain open.

## [Unreleased]

### Fixed

- Automatic observations preserve the active value entry and editor position;
  changed entry lists require a new selection before editing continues.
- Failed startup-path replacements retain ownership of the previous registration
  so enabling or disabling startup can be retried safely.
- Startup rejects commands exceeding the Windows Run limit before registering
  them or reporting startup as enabled, including paths containing Unicode.
- CI classifies Cargo test targets by exact case and rejects unclassified targets
  whose names differ only by capitalization from an existing suite.

### Added

- GitHub Actions release workflow: matching version tags build the Windows x64
  application, run headless-compatible functional tests, verify package contents
  and privacy checks, and publish a preview release with a ZIP and SHA-256 checksum.
- Manual workflow runs for build validation and downloadable artifacts without
  publishing a release.
- Release instructions in [RELEASING.md](RELEASING.md).

### Changed

- Build scripts automatically discover installed MSVC and Windows SDK locations,
  support installed-tool overrides, and respect custom Cargo cache locations.

## [0.1.2] - 2026-11-07

Initially built as a local candidate on 2026-11-04; published on GitHub on 2026-11-07.

### Added

- Custom application icon based on the supplied artwork, embedded in the executable
  for Windows Explorer, the main/history windows, and the normal system tray state.
- Transparent icon images at 16, 24, 32, 48, 64, 128 and 256 pixels.
- Local icon-conversion script and Windows SDK resource compilation.

### Changed

- Recording interruptions continue to use the error icon and status tooltip.
- Public release builds remap source paths to remove personal build-machine paths
  from compiled diagnostics. Public packages are scanned before publication.
- Public GitHub distribution uses a privacy-reviewed source snapshot, excluding
  internal planning/validation notes, local databases, caches and private history.

## 0.1.1 - 2026-11-03

Local candidate.

### Fixed

- Entry previews now display Windows path separators literally instead of doubling
  backslashes. The fix covers initial loading, live edits, Discard/reload and history
  comparisons, including UNC paths. Stored values and selected-entry text retain
  their exact content.

### Changed

- Package validation checks the version declared in Cargo.toml instead of a
  hard-coded version number.

## 0.1.0 - 2026-11-02

Initial complete local candidate.

### Added

- Portable Windows 11 x64 application built with Rust, native Windows controls and
  bundled SQLite, with settings and history stored in the adjacent `data/` folder.
- Separate user/system variable browsing, name/value search and scope filtering.
- Entry-by-entry editing for every variable, preserving entry order, whitespace,
  duplicates, empty entries and raw references such as `%SystemRoot%`.
- Direct Save, selected-draft Discard, variable creation and inline deletion
  confirmation. Session drafts survive navigation and minimizing to the tray.
- Preservation of supported plain/expandable registry types, with unsupported or
  malformed values displayed read-only.
- Variable change history for app-made edits, observed persistent external changes
  and net differences across monitoring gaps. External history is observational
  and can miss intermediate edits.
- Separate paged history window with selected/all-variable filters, before/after
  entry comparisons, and color/text labels for added, edited and deleted entries.
- Explicit completed-history clearing that preserves current variables, baselines,
  pending-operation recovery information and retained migration backups.
- Same-account administrator-consent helper for system-variable writes, with bounded
  authenticated requests and allowlisted registry operations.
- Pending write intents, conflict detection, readback and interruption recovery,
  with further edits blocked when recording is unavailable.
- Tray monitoring, reopening and explicit Exit; sign-in startup enabled by default
  after healthy initialization, with a setting to disable it.
- Inline Exit-and-discard / Cancel-exit choices when unsaved drafts exist.
- Sleep/resume observation-recovery handling and monitoring-gap reconciliation.
- Manual portable updates, verified pre-migration database backups, newer-schema
  rejection and repair of the application's owned enabled startup path after a move.
- Versioned ZIP packaging, dependency notices and run/update instructions.

[Unreleased]: https://github.com/karaikyo/environment-variable-manager/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/karaikyo/environment-variable-manager/releases/tag/v0.1.2
