# Wall time and peak working set of `trellis index` with the Pyright backstop.
#
# Method: the stopwatch spans process start to exit. Every 500 ms it reads
# PeakWorkingSet64 of the trellis process and of every node.exe whose command
# line contains "pyright". PeakWorkingSet64 is the OS's running maximum, so a
# sample only misses growth in the last interval before a process exits.
# The two processes are reported separately, never summed.
#
# Usage (PowerShell 7):
#   ./pyright_backstop_measure.ps1 -Exe <trellis.exe> -Root <repo> -Out <file> -PyrightBin <dir with pyright-langserver>
param([string]$Exe, [string]$Root, [string]$Out, [string]$PyrightBin)
# Only this script's children see the scratch Pyright install.
$env:PATH = $PyrightBin + ";" + $env:PATH
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Exe
$psi.Arguments = "--root `"$Root`" index"
$psi.WorkingDirectory = $Root
$psi.RedirectStandardOutput = $true
$psi.UseShellExecute = $false
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$p = [System.Diagnostics.Process]::Start($psi)
$stdoutTask = $p.StandardOutput.ReadToEndAsync()
$peak = @{}
while (-not $p.HasExited) {
  try { $p.Refresh(); $peak["trellis"] = [Math]::Max([int64]$peak["trellis"], $p.PeakWorkingSet64) } catch {}
  $kids = Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object { $_.CommandLine -like "*pyright*" }
  foreach ($k in $kids) {
    try { $gp = Get-Process -Id $k.ProcessId -ErrorAction Stop; $key = "node:" + $k.ProcessId; $peak[$key] = [Math]::Max([int64]$peak[$key], $gp.PeakWorkingSet64) } catch {}
  }
  Start-Sleep -Milliseconds 500
}
$sw.Stop()
$stdoutTask.Result | Set-Content -Path $Out
"exit=$($p.ExitCode) wall_s=$($sw.Elapsed.TotalSeconds)" | Add-Content -Path $Out
foreach ($k in $peak.Keys) { "peak_working_set $k = $([Math]::Round($peak[$k]/1MB,1)) MiB" | Add-Content -Path $Out }
