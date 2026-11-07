param([Parameter(Mandatory)][string]$Executable)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Executable)) { throw 'The release fixture executable must exist before launch can pass.' }
$source = (Resolve-Path -LiteralPath $Executable).Path
$runDirectory = Join-Path ([IO.Path]::GetTempPath()) ('evm-release-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $runDirectory | Out-Null
$copy = Join-Path $runDirectory 'browse-fixture.exe'
[IO.File]::WriteAllBytes($copy, [IO.File]::ReadAllBytes($source))
if ((Get-FileHash -LiteralPath $copy).Hash -ne (Get-FileHash -LiteralPath $source).Hash) { throw 'Release copy hash mismatch.' }
$timer = [Diagnostics.Stopwatch]::StartNew()
$process = Start-Process -FilePath $copy -WorkingDirectory $runDirectory -WindowStyle Normal -PassThru
try {
    if (-not $process.WaitForInputIdle(10000)) { throw 'No native input-idle window within ten seconds.' }
    $inputIdleMs = $timer.ElapsedMilliseconds
    do {
        Start-Sleep -Milliseconds 20
        $process.Refresh()
        if ($process.HasExited) { throw "Native application exited during loading." }
    } until ($process.MainWindowTitle -eq "Environment Manager" -or $timer.ElapsedMilliseconds -gt 10000)
    if ($process.MainWindowTitle -ne "Environment Manager") { throw "The native window did not report browse readiness." }
    $browseReadyMs = $timer.ElapsedMilliseconds
    Start-Sleep -Seconds 2
    $process.Refresh()
    if ($process.HasExited) { throw 'Native application exited before inspection.' }
    [pscustomobject]@{ InputIdleMilliseconds = $inputIdleMs; BrowseReadyMilliseconds = $browseReadyMs; PrivateBytes = $process.PrivateMemorySize64; WorkingSetBytes = $process.WorkingSet64; ExecutableBytes = (Get-Item $copy).Length; DataCreated = Test-Path -LiteralPath (Join-Path $runDirectory 'data/environment.sqlite'); MainWindowTitle = $process.MainWindowTitle; ExecutionDirectory = $runDirectory } | ConvertTo-Json
} finally {
    if (-not $process.HasExited) { $process.CloseMainWindow() | Out-Null; if (-not $process.WaitForExit(5000)) { $process.Kill(); $process.WaitForExit(); throw "Native window did not exit cleanly after Close." } }
    # Keep isolated evidence/database for a reopen check and explicit later cleanup.
}
