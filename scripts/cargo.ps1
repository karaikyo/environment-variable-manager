# Offline local toolchain adapter. Does not install tools or change persisted environment variables.
param([Parameter(ValueFromRemainingArguments)][string[]]$CargoArguments)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$rustBin = Join-Path $env:USERPROFILE '.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin'
$msvc = $env:EVM_MSVC_TOOLS
if (-not $msvc) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $installation = if (Test-Path -LiteralPath $vswhere) { & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
    if (-not $installation) { throw 'Installed MSVC tools not found; set EVM_MSVC_TOOLS.' }
    $toolsVersion = (Get-Content -LiteralPath (Join-Path $installation 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
    $msvc = Join-Path $installation "VC/Tools/MSVC/$toolsVersion"
}
$sdk = $env:EVM_WINDOWS_SDK_LIB
if (-not $sdk) {
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Lib'
    $sdk = Get-ChildItem -LiteralPath $sdkRoot -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um/x64/kernel32.lib') } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1 -ExpandProperty FullName
    if (-not $sdk) { throw 'Installed Windows SDK libraries not found; set EVM_WINDOWS_SDK_LIB.' }
}
if (-not (Test-Path -LiteralPath (Join-Path $rustBin 'rustc.exe'))) {
    $rustCompiler = & rustup which rustc
    if ($LASTEXITCODE -ne 0) { throw 'An installed Rust toolchain is required.' }
    $rustBin = Split-Path $rustCompiler -Parent
}
$msvc = $msvc.Replace('\','/')
$sdk = $sdk.Replace('\','/')
foreach ($required in @((Join-Path $rustBin 'cargo.exe'), (Join-Path $rustBin 'rustc.exe'), "$msvc/bin/Hostx64/x64/link.exe", "$sdk/um/x64/kernel32.lib")) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Required installed tool missing: $required. No download is attempted; update local paths in scripts/cargo.ps1." }
}
$sourceCargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$cache = Join-Path $root 'cargo-home/registry'
if (-not (Test-Path -LiteralPath (Join-Path $cache 'cache'))) {
    New-Item -ItemType Directory -Path $cache -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $sourceCargoHome 'registry/cache') -Destination $cache -Recurse
    Copy-Item -LiteralPath (Join-Path $sourceCargoHome 'registry/index') -Destination $cache -Recurse
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
