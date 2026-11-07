# Package-level contract: only redistributable application files leave the workspace.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path ([IO.Path]::GetTempPath()) ('evm-package-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $output | Out-Null
& (Join-Path $PSScriptRoot 'package.ps1') -OutputDirectory $output
if ($LASTEXITCODE -ne 0) { throw 'Package build failed' }
$archives = @(Get-ChildItem -LiteralPath $output -File -Filter '*.zip')
if ($archives.Count -ne 1) { throw 'Expected one versioned ZIP' }
$zip = [IO.Compression.ZipFile]::OpenRead($archives[0].FullName)
try {
    $paths = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\','/') })
    foreach ($required in @('environment-variable-manager.exe','VERSION.txt','RUN-UPDATE.md','THIRD-PARTY-NOTICES.md')) {
        if ($required -notin $paths) { throw "Missing distribution file: $required" }
    }
    foreach ($path in $paths) {
        if ($path -notin @('environment-variable-manager.exe','VERSION.txt','RUN-UPDATE.md','THIRD-PARTY-NOTICES.md') -and $path -notmatch '^notices/([^/]+/(LICENSE[^/]*|COPYRIGHT[^/]*|PUBLIC-DOMAIN[^/]*)|rust/licenses/[A-Za-z0-9.-]+\.txt)$') { throw "Unexpected distributed content: $path" }
    }
    if (-not ($paths -match '^notices/rust/')) { throw 'Rust runtime notices missing' }
    if (-not ($paths -match '^notices/libsqlite3-sys-0.39.1/')) { throw 'SQLite notices missing' }
    if (-not ($paths -match '^notices/windows-sys-0.61.2/')) { throw 'Windows bindings notices missing' }
    if (($zip.Entries | Measure-Object -Property Length -Sum).Sum -gt 50MB -or $archives[0].Length -gt 20MB) { throw 'Distribution exceeds accepted size budget' }
} finally { $zip.Dispose() }
$extract = Join-Path $output 'clean-extract'
[IO.Compression.ZipFile]::ExtractToDirectory($archives[0].FullName, $extract)
$exe = Join-Path $extract 'environment-variable-manager.exe'
if ((Get-FileHash -LiteralPath $exe).Hash -ne (Get-FileHash -LiteralPath (Join-Path $root 'target/x86_64-pc-windows-msvc/release/environment-variable-manager.exe')).Hash) { throw 'Extracted executable differs from release build' }
$version = Get-Content -LiteralPath (Join-Path $extract 'VERSION.txt') -Raw
$expectedVersion = [regex]::Match((Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw), '(?m)^version = "([0-9.]+)"\r?$').Groups[1].Value
if (-not $expectedVersion -or -not $version.Contains("Environment Manager $expectedVersion") -or $version -notmatch 'Windows 11 x64' -or $version -notmatch 'candidate') { throw 'Version/platform/candidate status missing' }
Write-Output "Package contract passed: $($archives[0].FullName)"
