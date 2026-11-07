# Continuous integration and automated preview releases

Every push to `main` and pull request targeting `main` runs Windows CI: Rust formatting,
Clippy, a safety-reviewed list of headless integration suites, and packaging checks.
`scripts/ci-test-targets.ps1` uses `cargo metadata` to audit every Cargo
integration-test target, including nested `tests/<name>/main.rs` targets and
explicit `[[test]]` entries. Unclassified new targets fail CI until explicitly
assigned to the headless group or the controlled-acceptance group. The controlled group currently contains
`cli_observation`, `history_native`, `native_lifecycle`, `recovery_process`,
and `system_helper`; these are not silently reported as passed in CI.
CI also verifies that inventory probes refuse pre-existing directories and preserve
their contents. Cleanup removes only fixtures successfully reserved by that run.

Push a tag matching Cargo.toml (for example v0.1.3 after updating the package and
lockfile versions) to run the Windows release workflow. It tests headless-compatible
functional suites on Windows Server 2022, checks the portable archive, scans it for
personal paths/credential patterns, and publishes a preview release with ZIP/SHA256.
Tag builds must use a version matching Cargo.toml and a commit reachable from
`main`. The release inspection hashes the executable directly from inside the ZIP
and compares it with the built binary and package metadata. The publish job
independently validates the uploaded ZIP against its SHA-256 sidecar. Its release notes come from the matching numbered section in
CHANGELOG.md and include the exact tagged source commit. Add the version heading
(e.g. `## [0.1.3] - YYYY-MM-DD`) before tagging or publication will fail.
Official actions are pinned to commit hashes. Release write permission belongs only
to the publish job; no custom release token or repository secret is required.

Actions > Windows release > Run workflow runs build/verification and retains the
ZIP/checksum as an Actions artifact for 14 days, without creating a release. Use
this to validate changes before pushing a version tag. Existing releases are not
replaced, and the tag must exactly match Cargo.toml.

GitHub's Windows runner provides Rust, MSVC and the Windows SDK. The workflow caches Cargo dependency downloads keyed to Cargo.lock and fetches
the Cargo.lock dependencies on that disposable runner, then builds offline. Local
scripts continue using existing installed tools/caches without downloads. MSVC/SDK
locations are detected automatically; EVM_MSVC_TOOLS and EVM_WINDOWS_SDK_LIB can
select installed alternatives. No software is installed on your PC by this workflow.

Native GUI/UAC tests depend on an unelevated interactive Windows session; the hosted
runner uses an elevated/headless server environment. The workflow runs the supported
functional/registry/icon suites, and leaves the complete native UI/helper session,
actual sleep/hibernate, accessibility and remaining manual acceptance to the
controlled acceptance procedure. Passing CI does not imply those checks passed.

To publish a future version:

1. Update Cargo.toml and the root package entry in Cargo.lock, promote the
   changes from `## [Unreleased]` to a matching numbered heading in CHANGELOG.md,
   and commit/push main. Wait for the normal CI checks.
2. Run the workflow manually and check its result.
3. Create and push the matching tag: git tag v0.1.3, then git push origin v0.1.3.
4. Check the workflow and Releases page. Releases are previews by default; promote
   a release only after the relevant acceptance checks.
