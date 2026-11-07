# Every tests/*.rs target must be classified. Unclassified tests fail CI.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
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
$actual = @(Get-ChildItem -LiteralPath (Join-Path $root 'tests') -File -Filter '*.rs' | ForEach-Object { $_.BaseName })
$classified = @($headless) + @($controlled.Keys)
$duplicates = @($classified | Group-Object | Where-Object Count -gt 1 | ForEach-Object Name)
$missing = @($actual | Where-Object { $_ -notin $classified })
$stale = @($classified | Where-Object { $_ -notin $actual })
if ($duplicates.Count -or $missing.Count -or $stale.Count) {
  throw "Integration-test inventory changed. Duplicates: $($duplicates -join ', '); unclassified: $($missing -join ', '); missing targets: $($stale -join ', '). Review test safety before updating the allowlist."
}
$headless
