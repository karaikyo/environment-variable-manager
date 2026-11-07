# Automated preview releases

Push a tag matching Cargo.toml (for example v0.1.3 after updating the package and
lockfile versions) to run the Windows release workflow. It tests headless-compatible
functional suites on Windows Server 2022, checks the portable archive, scans it for
personal paths/credential patterns, and publishes a preview release with ZIP/SHA256.
Official actions are pinned to commit hashes. Release write permission belongs only
to the publish job; no custom release token or repository secret is required.

Actions > Windows release > Run workflow runs build/verification and retains the
ZIP/checksum as an Actions artifact for 14 days, without creating a release. Use
this to validate changes before pushing a version tag. Existing releases are not
replaced, and the tag must exactly match Cargo.toml.

GitHub's Windows runner provides Rust, MSVC and the Windows SDK. The workflow fetches
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

1. Update Cargo.toml and the root package entry in Cargo.lock; commit and push main.
2. Run the workflow manually and check its result.
3. Create and push the matching tag: git tag v0.1.3, then git push origin v0.1.3.
4. Check the workflow and Releases page. Releases are previews by default; promote
   a release only after the relevant acceptance checks.
