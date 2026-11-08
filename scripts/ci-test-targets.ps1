# Classify every Cargo integration-test target, including nested and explicit [[test]] targets.
param([string]$Root = (Split-Path $PSScriptRoot -Parent))
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Root).Path
$headless = @(
  'browse', 'create_delete', 'edits', 'history', 'lifecycle',
  'observation', 'recovery', 'resume', 'startup', 'system_edits',
  'updates', 'windows_registry', 'icons'
)
$controlled = @{
  cli_observation = 'Uses real HKCU Environment and reg.exe; needs a controlled account'
  history_native = 'Native windows and message-loop scenarios require interactive acceptance'
  native_lifecycle = 'Native desktop lifecycle requires interactive acceptance'
  recovery_process = 'Process termination and HKCU fixtures need an isolated session'
  system_helper = 'Named pipe, identity and helper security checks need controlled acceptance'
}
# Cargo is authoritative: tests/<name>/main.rs, [[test]] and autobins settings
# cannot be inferred reliably from a tests/*.rs directory listing.
$manifest = Join-Path $root 'Cargo.toml'
$metadataJson = & cargo metadata --manifest-path $manifest --format-version 1 --no-deps --offline --locked
if ($LASTEXITCODE -ne 0 -or -not $metadataJson) { throw 'Failed to enumerate Cargo test targets' }
$metadata = $metadataJson | ConvertFrom-Json -Depth 100
$rootManifest = [IO.Path]::GetFullPath($manifest)
$packages = @($metadata.packages | Where-Object { [IO.Path]::GetFullPath($_.manifest_path) -eq $rootManifest })
if ($packages.Count -ne 1) { throw "Expected exactly one root Cargo package, found $($packages.Count)" }
$actual = @($packages[0].targets | Where-Object { 'test' -in @($_.kind) } | ForEach-Object { $_.name })
if (-not $actual.Count) { throw 'Cargo reported no integration-test targets' }
$classified = @($headless) + @($controlled.Keys)
$duplicates = @($classified | Group-Object -CaseSensitive | Where-Object Count -gt 1 | ForEach-Object Name)
$missing = @($actual | Where-Object { $_ -cnotin $classified })
$stale = @($classified | Where-Object { $_ -cnotin $actual })
if ($duplicates.Count -or $missing.Count -or $stale.Count) {
  throw "Integration-test inventory changed. Duplicates: $($duplicates -join ', '); unclassified: $($missing -join ', '); missing targets: $($stale -join ', '). Review test safety before updating the allowlist."
}
$headless
