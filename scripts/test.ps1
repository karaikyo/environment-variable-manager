param([string]$Filter = '')
$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    $cargo = Join-Path $PSScriptRoot 'cargo.ps1'
    $output = & $cargo test --offline --locked --no-run --message-format=json
    $compileExit = $LASTEXITCODE
    $output | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-message' } | ForEach-Object { Write-Host $_.message.rendered }
    if ($compileExit -ne 0) { throw 'Test compilation failed.' }
    $executables = $output | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } | ForEach-Object { $_.executable }
    foreach ($source in $executables) {
        # Ordinary-user execution copy: do not change the source security label.
        $runDirectory = Join-Path ([IO.Path]::GetTempPath()) ('evm-tests-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $runDirectory | Out-Null
        $copy = Join-Path $runDirectory ([IO.Path]::GetFileName($source))
        try {
            [IO.File]::WriteAllBytes($copy, [IO.File]::ReadAllBytes($source))
            if ((Get-FileHash -LiteralPath $copy).Hash -ne (Get-FileHash -LiteralPath $source).Hash) { throw 'Test copy hash mismatch.' }
            & $copy $Filter --nocapture
            if ($LASTEXITCODE -ne 0) { throw "Test executable failed: $source" }
        } finally {
            Remove-Item -LiteralPath $copy -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $runDirectory
        }
    }
} finally { Pop-Location }
