param([switch]$Fixtures)
$arguments = @('build', '--release', '--offline', '--locked')
if ($Fixtures) { $arguments += '--examples' }
& (Join-Path $PSScriptRoot 'cargo.ps1') @arguments
exit $LASTEXITCODE
