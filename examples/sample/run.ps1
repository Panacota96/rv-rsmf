# Converts every synthetic sample export in one run on Windows and checks the result.
# Fails (non-zero exit) if the self-test, the conversion, verify-log or validate fails, or if the tool's hash
# register (logs\<RUN-ID>\hashes.csv) differs from the independently computed expected_hashes.csv.
# Usage: .\examples\sample\run.ps1 [-Bin C:\path\to\rv-rsmf.exe]
param([string]$Bin = "rv-rsmf")
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$out = Join-Path $here "out"
$evidence = Join-Path $here "evidence"
& $Bin self-test; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Times in the WhatsApp and CSV samples have no UTC offset: they are read as Paris time. The ambiguous WhatsApp
# sample (every day and month <= 12) is read as day/month/year. Expected exit code 0 (no high-severity finding).
& $Bin convert --input $evidence --recursive --timezone Europe/Paris --date-order dmy --hash sha256,md5 --lang all --case DEMO-001 --actor "Sample" --out $out --progress none
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$run = Get-ChildItem (Join-Path $out "logs") | Sort-Object Name | Select-Object -Last 1
& $Bin verify-log $run.FullName; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $Bin validate (Join-Path (Join-Path $out "rsmf") $run.Name); if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Compare the tool's hash register with expected_hashes.csv: same files (path below evidence\), sizes, SHA-256, MD5.
$expected = Import-Csv (Join-Path $here "expected_hashes.csv")
$register = Import-Csv (Join-Path $run.FullName "hashes.csv")
$prefix = (Resolve-Path $evidence).Path.TrimEnd('\') + '\'
$bad = 0
if ($register.Count -ne $expected.Count) {
  Write-Host "MISMATCH: the tool hashed $($register.Count) files, expected_hashes.csv lists $($expected.Count)"
  $bad = 1
}
foreach ($row in $expected) {
  $got = $register | Where-Object { $_.path.Replace($prefix, '').Replace('\', '/') -eq $row.file }
  if ($null -eq $got -or @($got).Count -ne 1) { Write-Host "MISMATCH: $($row.file) not in hashes.csv"; $bad = 1; continue }
  if ($got.size -ne $row.size -or $got.sha256 -ne $row.sha256 -or $got.md5 -ne $row.md5) {
    Write-Host "MISMATCH $($row.file): tool $($got.size),$($got.sha256),$($got.md5), expected $($row.size),$($row.sha256),$($row.md5)"
    $bad = 1
  }
}
if ($bad) { exit 3 }
Write-Host "hashes.csv matches expected_hashes.csv ($($expected.Count) files: size, SHA-256, MD5)"
Write-Host "RSMF files: $out\rsmf\$($run.Name)"
Write-Host "Reports:    $out\reports\$($run.Name)\statistics.en.html"
