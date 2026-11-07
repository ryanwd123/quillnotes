# Measures a release build for RELEASE_METRICS.md: startup time, memory and idle CPU.
# Usage: pwsh scripts/measure.ps1 [-Exe path\to\quillnotes.exe] [-File note.md]
# Settings go to a throwaway APPDATA/LOCALAPPDATA so your real session is not touched.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\quillnotes.exe",
    [string]$File = "$PSScriptRoot\..\sample.md",
    [int]$Runs = 7,
    [int]$IdleSeconds = 10
)

$tmp = Join-Path ([IO.Path]::GetTempPath()) "quillnotes-measure"
New-Item -ItemType Directory -Force "$tmp\appdata", "$tmp\localappdata" | Out-Null
$env:APPDATA = "$tmp\appdata"
$env:LOCALAPPDATA = "$tmp\localappdata"

function Stop-Quill($p) {
    $null = $p.CloseMainWindow()
    $null = $p.WaitForExit(5000)
    if (!$p.HasExited) { $p.Kill() }
}

# Startup: process launch until the main window exists and the message loop is idle.
$times = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process $Exe -ArgumentList "`"$File`"" -PassThru
    $null = $p.WaitForInputIdle(10000)
    while ($p.MainWindowHandle -eq 0 -and $sw.ElapsedMilliseconds -lt 10000) {
        Start-Sleep -Milliseconds 2
        $p.Refresh()
    }
    $times += $sw.Elapsed.TotalMilliseconds
    if ($i -lt $Runs - 1) { Stop-Quill $p }
}

# Idle: leave the last instance alone, then sample CPU time and memory.
Start-Sleep 3
$p.Refresh()
$cpu0 = $p.TotalProcessorTime
Start-Sleep $IdleSeconds
$p.Refresh()
$cpu = ($p.TotalProcessorTime - $cpu0).TotalMilliseconds

$s = $times | Sort-Object
"Binary size:   {0:N0} bytes" -f (Get-Item $Exe).Length
"Startup (ms):  median {0:N1}, min {1:N1}, max {2:N1} over $Runs runs" -f $s[[int]($Runs / 2)], $s[0], $s[-1]
"Memory (MB):   private {0:N1}, working set {1:N1}" -f ($p.PrivateMemorySize64 / 1MB), ($p.WorkingSet64 / 1MB)
"Idle CPU:      {0:N1} ms over $IdleSeconds s" -f $cpu
"Threads:       $($p.Threads.Count), handles: $($p.HandleCount)"
Stop-Quill $p
