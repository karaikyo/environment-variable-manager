param([Parameter(Mandatory)][string]$Fixture,[switch]$Resume,[switch]$LongRun)
$ErrorActionPreference='Stop'
$evidence=Join-Path ([IO.Path]::GetTempPath()) ('evm-release-reference-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $evidence | Out-Null
$copy=Join-Path $evidence 'lifecycle-fixture.exe'
[IO.File]::WriteAllBytes($copy,[IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Fixture).Path))
if ((Get-FileHash -LiteralPath $Fixture).Hash -ne (Get-FileHash -LiteralPath $copy).Hash) { throw 'Reference fixture execution copy mismatch' }
$arguments=@((Join-Path $evidence 'measurement'))
if ($Resume) { $arguments+='--resume' } else { $arguments+='--release-benchmark' }
if ($LongRun) { $arguments+='--long-run' }
& $copy @arguments
if ($LASTEXITCODE -ne 0) { throw "Reference measurement failed; evidence retained at $evidence" }
$os=Get-CimInstance Win32_OperatingSystem
$cpu=Get-CimInstance Win32_Processor
$computer=Get-CimInstance Win32_ComputerSystem
[pscustomobject]@{EvidenceDirectory=$evidence;FixtureSha256=(Get-FileHash -LiteralPath $copy).Hash;Windows=$os.Caption;Build=$os.BuildNumber;Architecture=$os.OSArchitecture;Cpu=($cpu.Name -join ', ');LogicalProcessors=$computer.NumberOfLogicalProcessors;RamBytes=$computer.TotalPhysicalMemory;Variables=200;Entries=2000;History=10000;ProcessCount=1;SimulatedResume=$true;ChangedGapScenario=[bool]$Resume;LongerRun=[bool]$LongRun;Limitation='Shared production UI/worker/watchers with owned registry/startup adapter and fixture bookkeeping; not byte-identical to packaged product; actual power transition not performed'} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $evidence 'result.json')
