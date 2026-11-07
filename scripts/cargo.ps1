# Offline local toolchain adapter. Does not install tools or change persisted environment variables.
param([Parameter(ValueFromRemainingArguments)][string[]]$CargoArguments)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$rustBin = Join-Path $env:USERPROFILE '.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
$msvc = 'D:/dev/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC/14.42.34433'
$sdk = 'C:/Program Files (x86)/Windows Kits/10/Lib/10.0.22621.0'
foreach ($required in @((Join-Path $rustBin 'cargo.exe'), (Join-Path $rustBin 'rustc.exe'), "$msvc/bin/Hostx64/x64/link.exe", "$sdk/um/x64/kernel32.lib")) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Required installed tool missing: $required. No download is attempted; update local paths in scripts/cargo.ps1." }
}
$cache = Join-Path $root 'cargo-home/registry'
if (-not (Test-Path -LiteralPath (Join-Path $cache 'cache'))) {
    New-Item -ItemType Directory -Path $cache -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $env:USERPROFILE '.cargo/registry/cache') -Destination $cache -Recurse
    Copy-Item -LiteralPath (Join-Path $env:USERPROFILE '.cargo/registry/index') -Destination $cache -Recurse
}
$start = [Diagnostics.ProcessStartInfo]::new((Join-Path $rustBin 'cargo.exe'))
$start.WorkingDirectory = $root
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
# Child-process build configuration only; the caller environment is untouched.
$start.Environment['CARGO_HOME'] = Join-Path $root 'cargo-home'
# Remove personal build-machine paths from panic/source strings in shipped binaries.
$pathMaps = @(
    ('--remap-path-prefix=' + $root + '=/workspace/environment-variable-manager'),
    ('--remap-path-prefix=' + $root.Replace('\','/') + '=/workspace/environment-variable-manager'),
    ('--remap-path-prefix=' + $env:USERPROFILE + '=/user'),
    ('--remap-path-prefix=' + $env:USERPROFILE.Replace('\','/') + '=/user')
)
$configs = @(
    ('build.rustc="' + (Join-Path $rustBin 'rustc.exe').Replace('\','/') + '"'),
    ('target.x86_64-pc-windows-msvc.linker="' + "$msvc/bin/Hostx64/x64/link.exe" + '"'),
    ('target.x86_64-pc-windows-msvc.rustflags=' + (ConvertTo-Json -Compress -InputObject (@('-C','target-feature=+crt-static','-L',"native=$msvc/lib/x64",'-L',"native=$sdk/um/x64",'-L',"native=$sdk/ucrt/x64") + $pathMaps)))
)
foreach ($config in $configs) { $start.ArgumentList.Add('--config'); $start.ArgumentList.Add($config) }
foreach ($argument in $CargoArguments) { $start.ArgumentList.Add($argument) }
$process = [Diagnostics.Process]::Start($start)
$stdout = $process.StandardOutput.ReadToEndAsync()
$stderr = $process.StandardError.ReadToEndAsync()
$process.WaitForExit()
[Console]::Error.Write($stderr.GetAwaiter().GetResult())
($stdout.GetAwaiter().GetResult() -split "\r?\n") | Where-Object { $_.Length -gt 0 } | Write-Output
exit $process.ExitCode
