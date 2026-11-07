param([Parameter(Mandatory)][string]$OldExecutable,[Parameter(Mandatory)][string]$NewExecutable,[Parameter(Mandatory)][string]$Diagnostic)
$ErrorActionPreference='Stop'
$evidence=Join-Path ([IO.Path]::GetTempPath()) ('evm-manual-update-'+[guid]::NewGuid().ToString('N'))
$run=Join-Path $evidence 'portable'
New-Item -ItemType Directory -Path $run | Out-Null
$diagnosticCopy=Join-Path $evidence 'update-fixture.exe'
$app=Join-Path $run 'environment-variable-manager.exe'
function Copy-Verified([string]$source,[string]$destination) {
 [IO.File]::WriteAllBytes($destination,[IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $source).Path))
 if ((Get-FileHash -LiteralPath $source).Hash -ne (Get-FileHash -LiteralPath $destination).Hash) { throw 'Execution copy mismatch' }
}
Copy-Verified $Diagnostic $diagnosticCopy
Copy-Verified $OldExecutable $app
& $diagnosticCopy seed (Join-Path $run 'data')
if ($LASTEXITCODE -ne 0) { throw 'Seed failed' }
function Invoke-UpdateLaunch([string]$phase) {
 & $diagnosticCopy launch $app
 if ($LASTEXITCODE -ne 0) { throw "$phase native fixture failed" }
 Write-Output "$phase clean native startup and explicit Exit"
}
Invoke-UpdateLaunch 'Old schema1 product'
$reference=Join-Path $evidence 'reference'
New-Item -ItemType Directory -Path $reference | Out-Null
Copy-Item -LiteralPath (Join-Path $run 'data/environment.sqlite') -Destination (Join-Path $reference 'environment.sqlite')
$oldHash=(Get-FileHash -LiteralPath $app).Hash
Copy-Verified $NewExecutable $app
$newHash=(Get-FileHash -LiteralPath $app).Hash
if ($oldHash -eq $newHash) { throw 'Replacement did not change executable' }
Invoke-UpdateLaunch 'Updated schema2 product'
if (-not (Test-Path -LiteralPath (Join-Path $run 'data/before-schema-1-to-2.sqlite'))) { throw 'Migration backup missing' }
& $diagnosticCopy compare (Join-Path $run 'data') $reference
if ($LASTEXITCODE -ne 0) { throw 'Preservation comparison failed' }
[pscustomobject]@{EvidenceDirectory=$evidence;OldSha256=$oldHash;NewSha256=$newHash;NewBytes=(Get-Item -LiteralPath $app).Length;DatabaseBytes=(Get-Item -LiteralPath (Join-Path $run 'data/environment.sqlite')).Length;BackupBytes=(Get-Item -LiteralPath (Join-Path $run 'data/before-schema-1-to-2.sqlite')).Length} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $evidence 'result.json')
