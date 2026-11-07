param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Invalid release version' }
& (Join-Path $PSScriptRoot 'package.ps1')
if ($LASTEXITCODE -ne 0) { throw 'Packaging failed' }
$archive = Join-Path $root "dist/environment-variable-manager-$Version-windows-x64.zip"
$metadata = Get-Content -LiteralPath (Join-Path $root "dist/environment-variable-manager-$Version-windows-x64.json") -Raw | ConvertFrom-Json
if ($metadata.SourceRevision -cne (git -C $root rev-parse HEAD)) { throw 'Packaged source revision does not match checkout' }
if ($metadata.Archive -ne $archive) { throw 'Unexpected release archive path' }
if ($metadata.ExecutableSha256 -ne (Get-FileHash -LiteralPath (Join-Path $root 'target/x86_64-pc-windows-msvc/release/environment-variable-manager.exe') -Algorithm SHA256).Hash) { throw 'Packaged executable digest differs from built executable' }
if ($metadata.Dirty) { throw 'Release source must be clean' }
$zip = [IO.Compression.ZipFile]::OpenRead($archive)
try {
    $expected = @('environment-variable-manager.exe','VERSION.txt') + @(Get-Content -LiteralPath (Join-Path $PSScriptRoot 'package-files.txt'))
    $actual = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\','/') })
    if (Compare-Object ($expected | Sort-Object) ($actual | Sort-Object)) { throw 'Unexpected archive content' }
    $versionEntry = $zip.GetEntry('VERSION.txt')
    if (-not $versionEntry) { throw 'VERSION metadata missing from archive' }
    $reader = [IO.StreamReader]::new($versionEntry.Open())
    try { $versionText = $reader.ReadToEnd() } finally { $reader.Dispose() }
    if (-not $versionText.Contains("Environment Manager $Version") -or -not $versionText.Contains("Source revision: $($metadata.SourceRevision)")) { throw 'Embedded release provenance does not match metadata' }
    foreach ($entry in $zip.Entries) {
        if ($entry.FullName.Replace('\','/').StartsWith('notices/')) { continue }
        $stream = $entry.Open(); $memory = [IO.MemoryStream]::new()
        try {
            $stream.CopyTo($memory); $bytes = $memory.ToArray()
            foreach ($encoding in @([Text.Encoding]::ASCII,[Text.Encoding]::Unicode)) {
                $text = $encoding.GetString($bytes)
                $patterns = @('(?i)[a-z]:[\\/]Users[\\/][^\\/\s]+','(?i)[a-z]:[\\/](?:a|workspaces|home)[\\/]','github_pat_[A-Za-z0-9_]+','gh[pousr]_[A-Za-z0-9]{20,}','AKIA[0-9A-Z]{16}','BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY')
                foreach ($pattern in $patterns) { if ($text -match $pattern) { throw "Privacy scan failed in $($entry.FullName); sensitive match withheld" } }
            }
        } finally { $stream.Dispose(); $memory.Dispose() }
    }
} finally { $zip.Dispose() }
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
if ($hash -ne $metadata.Sha256) { throw 'Archive checksum mismatch' }
($hash + '  ' + [IO.Path]::GetFileName($archive)) | Set-Content -LiteralPath ($archive + '.sha256') -Encoding ascii
Write-Output "Release assets verified for version $Version"
