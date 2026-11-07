param([Parameter(Mandatory)][string]$Archive,[Parameter(Mandatory)][string]$Diagnostic,[switch]$Sample)
$ErrorActionPreference='Stop'
$evidence=Join-Path ([IO.Path]::GetTempPath()) ('evm-package-smoke-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $evidence | Out-Null
$portable=Join-Path $evidence 'clean-extract'
[IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path -LiteralPath $Archive).Path,$portable)
$diagnosticCopy=Join-Path $evidence 'update-fixture.exe'
[IO.File]::WriteAllBytes($diagnosticCopy,[IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Diagnostic).Path))
if ((Get-FileHash -LiteralPath $diagnosticCopy).Hash -ne (Get-FileHash -LiteralPath $Diagnostic).Hash) { throw 'Diagnostic execution copy mismatch' }
$app=Join-Path $portable 'environment-variable-manager.exe'
# Only disposable data is initialized; do not register the real product at sign-in.
& $diagnosticCopy seed-clean (Join-Path $portable 'data')
if ($LASTEXITCODE -ne 0) { throw 'Clean seed failed' }
for ($attempt=1; $attempt -le 5; $attempt++) {
    & $diagnosticCopy launch-timing $app (Join-Path $evidence "warm-$attempt")
    if ($LASTEXITCODE -ne 0) { throw 'Packaged native startup/Exit failed' }
}
if ($Sample) {
    & $diagnosticCopy launch-sample $app (Join-Path $evidence 'product-tray')
    if ($LASTEXITCODE -ne 0) { throw 'Packaged idle sampling failed' }
}
[pscustomobject]@{EvidenceDirectory=$evidence;ArchiveSha256=(Get-FileHash -LiteralPath $Archive).Hash;ProductSha256=(Get-FileHash -LiteralPath $app).Hash;DiagnosticSha256=(Get-FileHash -LiteralPath $diagnosticCopy).Hash;Sampled=[bool]$Sample;ProcessCount=1;Dataset='Real Windows values, disposable data, startup disabled';Timing='Warm-cache process launch to native ready title and healthy observation status; not cold-start or visual proof'} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $evidence 'result.json')
