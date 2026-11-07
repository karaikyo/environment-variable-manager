param([Parameter(Mandatory)][string]$Executable)
$ErrorActionPreference = 'Stop'
$source = (Resolve-Path -LiteralPath $Executable).Path
$runDirectory = Join-Path ([IO.Path]::GetTempPath()) ('evm-history-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $runDirectory | Out-Null
$copy = Join-Path $runDirectory 'history-fixture.exe'
[IO.File]::WriteAllBytes($copy, [IO.File]::ReadAllBytes($source))
if ((Get-FileHash -LiteralPath $copy).Hash -ne (Get-FileHash -LiteralPath $source).Hash) { throw 'History fixture hash mismatch.' }
# Visible developer-only window, explicitly authorized lifecycle smoke. No input,
# accessibility automation, pre-existing Windows variables or real data involved.
$process = Start-Process -FilePath $copy -WorkingDirectory $runDirectory -WindowStyle Normal -PassThru
if (-not $process.WaitForExit(45000)) { $process.Kill(); $process.WaitForExit(); throw 'History lifecycle fixture timed out.' }
if ($process.ExitCode -ne 0) { throw "History lifecycle fixture failed: $($process.ExitCode)" }
Get-Content -LiteralPath (Join-Path $runDirectory 'history-fixture-data/memory.json')
Write-Output "Evidence directory: $runDirectory"
