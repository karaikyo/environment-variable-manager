# Offline Windows x64 candidate packaging. Never includes application data.
param([string]$OutputDirectory = (Join-Path (Split-Path $PSScriptRoot -Parent) 'dist'))
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
& (Join-Path $PSScriptRoot 'build.ps1')
if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
$manifest = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
$version = [regex]::Match($manifest, '(?m)^version = "([0-9.]+)"\r?$').Groups[1].Value
if (-not $version) { throw 'Missing package version' }
$name = "environment-variable-manager-$version-windows-x64"
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDirectory).Path
$archive = Join-Path $output "$name.zip"
if (Test-Path -LiteralPath $archive) { throw 'Archive exists; choose a new output directory to preserve prior evidence' }
$stage = Join-Path $output ($name + '-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
$exe = Join-Path $root 'target/x86_64-pc-windows-msvc/release/environment-variable-manager.exe'
Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'environment-variable-manager.exe')
foreach ($file in Get-Content -LiteralPath (Join-Path $PSScriptRoot 'package-files.txt')) {
    if ($file -notmatch '^(RUN-UPDATE\.md|THIRD-PARTY-NOTICES\.md|notices/[A-Za-z0-9./_-]+)$' -or $file.Contains('..')) { throw "Unsafe manifest entry: $file" }
    $destination = Join-Path $stage $file
    New-Item -ItemType Directory -Path (Split-Path $destination -Parent) -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $root $file) -Destination $destination
}
$revision = git -C $root rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify source revision' }
$dirty = [bool](git -C $root status --porcelain --untracked-files=normal)
@"
Environment Manager $version
Platform: Windows 11 x64
Status: local release candidate; manual acceptance checks pending
Source revision: $revision
Uncommitted source changes: $dirty
Build: release, bundled SQLite, static C runtime, offline dependencies
Executable SHA256: $((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash)
"@ | Set-Content -LiteralPath (Join-Path $stage 'VERSION.txt')
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $archive, [IO.Compression.CompressionLevel]::Optimal, $false)
$size = (Get-ChildItem -LiteralPath $stage -File -Recurse | Measure-Object -Property Length -Sum).Sum
if ((Get-Item -LiteralPath $archive).Length -gt 20MB -or $size -gt 50MB) { throw 'Distribution exceeds accepted size budget' }
[pscustomobject]@{ Archive=$archive; Sha256=(Get-FileHash -LiteralPath $archive).Hash; CompressedBytes=(Get-Item -LiteralPath $archive).Length; ExtractedBytes=$size; ExecutableSha256=(Get-FileHash -LiteralPath $exe).Hash; SourceRevision=$revision; Dirty=$dirty } | ConvertTo-Json | Tee-Object -FilePath (Join-Path $output "$name.json")
