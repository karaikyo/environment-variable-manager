# Negative regression checks for Cargo test targets missed by file-only inventory.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$inventory = Join-Path $PSScriptRoot 'ci-test-targets.ps1'
$probe = 'evm_inventory_unclassified_probe'
$nested = Join-Path $root "tests/$probe/main.rs"
$explicit = Join-Path $root "tests/$probe-explicit.rs"
$manifest = Join-Path $root 'Cargo.toml'
$manifestBytes = [IO.File]::ReadAllBytes($manifest)
if ((Test-Path -LiteralPath $nested) -or (Test-Path -LiteralPath $explicit)) {
    throw 'Inventory regression fixture already exists; refusing to replace existing files'
}
function Assert-Unclassified([string]$Name) {
    $rejected = $false
    try { & $inventory -Root $root | Out-Null }
    catch {
        if ($_.Exception.Message -notlike "*$Name*") { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw "Inventory silently accepted unclassified Cargo test target: $Name" }
}
try {
    New-Item -ItemType Directory -Path (Split-Path $nested -Parent) | Out-Null
    [IO.File]::WriteAllText($nested, '#[test] fn detected_by_cargo() {}')
    Assert-Unclassified $probe
    Remove-Item -LiteralPath (Split-Path $nested -Parent) -Recurse

    [IO.File]::WriteAllText($explicit, '#[test] fn detected_by_cargo() {}')
    [IO.File]::AppendAllText($manifest, "`n[[test]]`nname = `"$probe-explicit`"`npath = `"tests/$probe-explicit.rs`"`n")
    Assert-Unclassified "$probe-explicit"
}
finally {
    [IO.File]::WriteAllBytes($manifest, $manifestBytes)
    if (Test-Path -LiteralPath (Split-Path $nested -Parent)) {
        Remove-Item -LiteralPath (Split-Path $nested -Parent) -Recurse -Force
    }
    if (Test-Path -LiteralPath $explicit) { Remove-Item -LiteralPath $explicit -Force }
}
# Confirm the working tree is back to its original test-target inventory.
& $inventory -Root $root | Out-Null
Write-Output 'Cargo test inventory rejection regression checks passed'
