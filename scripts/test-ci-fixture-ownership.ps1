# Exercise the real inventory-regression command against a pre-existing directory.
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('evm-ci-ownership-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    $scripts = Join-Path $fixture 'scripts'
    $existing = Join-Path $fixture 'tests/evm_inventory_unclassified_probe'
    New-Item -ItemType Directory -Path $scripts,$existing | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'test-ci-inventory.ps1') -Destination $scripts
    [IO.File]::WriteAllText((Join-Path $scripts 'ci-test-targets.ps1'), "throw 'Inventory must not run before refusing a pre-existing fixture'")
    $manifest = Join-Path $fixture 'Cargo.toml'
    $sentinel = Join-Path $existing 'keep.txt'
    [IO.File]::WriteAllText($manifest, '# Original manifest sentinel')
    [IO.File]::WriteAllText($sentinel, 'Existing contents must survive')
    $beforeManifest = [IO.File]::ReadAllBytes($manifest)
    $rejected = $false
    try { & (Join-Path $scripts 'test-ci-inventory.ps1') | Out-Null }
    catch { $rejected = $true }
    if (-not $rejected) { throw 'Pre-existing fixture directory was accepted' }
    if (-not (Test-Path -LiteralPath $sentinel)) { throw 'Regression: pre-existing sentinel was deleted' }
    if ([IO.File]::ReadAllText($sentinel) -cne 'Existing contents must survive') { throw 'Existing sentinel was modified' }
    if ([Convert]::ToBase64String([IO.File]::ReadAllBytes($manifest)) -cne [Convert]::ToBase64String($beforeManifest)) { throw 'Existing manifest was modified' }
    Write-Output 'Pre-existing fixture directory, sentinel and manifest preserved'
} finally {
    $resolvedFixture = (Resolve-Path -LiteralPath $fixture).Path
    $tempRoot = (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).Path.TrimEnd('\','/')
    if (-not $resolvedFixture.StartsWith($tempRoot + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Cleanup target outside owned temporary root' }
    Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}
