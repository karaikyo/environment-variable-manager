# Negative regression checks for Cargo test targets missed by file-only inventory.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Split-Path $PSScriptRoot -Parent)).Path
$inventory = Join-Path $PSScriptRoot 'ci-test-targets.ps1'
$probe = 'evm_inventory_unclassified_probe'
$nestedDirectory = Join-Path $root "tests/$probe"
$nested = Join-Path $nestedDirectory 'main.rs'
$explicit = Join-Path $root "tests/$probe-explicit.rs"
$manifest = Join-Path $root 'Cargo.toml'
$manifestBytes = [IO.File]::ReadAllBytes($manifest)
if ((Test-Path -LiteralPath $nestedDirectory) -or (Test-Path -LiteralPath $explicit)) {
    throw 'Inventory regression fixture already exists; refusing to replace existing files or directories'
}
$ownsNested = $false
$ownsExplicit = $false
$manifestChanged = $false
function Assert-Unclassified([string]$Name) {
    $rejected = $false
    try { & $inventory -Root $root | Out-Null }
    catch {
        if ($_.Exception.Message -cnotlike "*$Name*") { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw "Inventory silently accepted unclassified Cargo test target: $Name" }
}
try {
    # No -Force: a collision after the initial check must fail without claiming ownership.
    New-Item -ItemType Directory -Path $nestedDirectory | Out-Null
    $ownsNested = $true
    [IO.File]::WriteAllText($nested, '#[test] fn detected_by_cargo() {}')
    Assert-Unclassified $probe
    $resolved = (Resolve-Path -LiteralPath $nestedDirectory).Path
    if (-not $resolved.StartsWith($root + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture cleanup path' }
    Remove-Item -LiteralPath $resolved -Recurse
    $ownsNested = $false

    $file = [IO.File]::Open($explicit,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    $ownsExplicit = $true
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes('#[test] fn detected_by_cargo() {}')
        $file.Write($bytes,0,$bytes.Length)
    } finally { $file.Dispose() }
    $manifestChanged = $true
    [IO.File]::AppendAllText($manifest, "`n[[test]]`nname = `"$probe-explicit`"`npath = `"tests/$probe-explicit.rs`"`n")
    Assert-Unclassified "$probe-explicit"

    # Cargo names are case-sensitive even on Windows. A distinct explicit target
    # must not inherit the existing lowercase history target's classification.
    [IO.File]::WriteAllBytes($manifest, $manifestBytes)
    [IO.File]::AppendAllText($manifest, "`n[[test]]`nname = `"History`"`npath = `"tests/$probe-explicit.rs`"`n")
    Assert-Unclassified 'History'
}
finally {
    if ($manifestChanged) { [IO.File]::WriteAllBytes($manifest, $manifestBytes) }
    if ($ownsNested -and (Test-Path -LiteralPath $nestedDirectory)) {
        $resolved = (Resolve-Path -LiteralPath $nestedDirectory).Path
        if (-not $resolved.StartsWith($root + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture cleanup path' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
    if ($ownsExplicit -and (Test-Path -LiteralPath $explicit)) { Remove-Item -LiteralPath $explicit -Force }
}
# Confirm the working tree is back to its original test-target inventory.
& $inventory -Root $root | Out-Null
Write-Output 'Cargo test inventory rejection regression checks passed'
