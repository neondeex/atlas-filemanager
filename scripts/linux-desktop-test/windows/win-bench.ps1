# Atlas on Windows: the same measurements as perf.sh / frames.sh on Linux.
# Runs inside the logged-on desktop session (schtasks /IT), never in the SSH session 0.
#   win-bench.ps1 -Mode setup|cold|main|frames -Exe C:\...\atlas.exe -Out C:\Users\Public\atlas-cmp-run\out
# Atlas runs in a 1366x768 window like the Linux VMs. Data lives under C:\Users\Public\atlas-cmp-run.
param([string]$Mode, [string]$Exe, [string]$Out)
$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture   # 1.23, not 1,23
$R = 'C:\Users\Public\atlas-cmp-run'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$results = Join-Path $Out 'results.txt'
function Log($s) { Add-Content -Path $results -Value $s -Encoding ascii }
trap { Log "ERROR line $($_.InvocationInfo.ScriptLineNumber): $_"; break }

Add-Type -ReferencedAssemblies System.Drawing @"
using System; using System.Runtime.InteropServices; using System.Threading; using System.Drawing;
public static class W {
  [StructLayout(LayoutKind.Sequential)] struct MI { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk, scan; public uint flags, time; public IntPtr extra; public int pad1, pad2; }
  [StructLayout(LayoutKind.Explicit)] struct IN { [FieldOffset(0)] public uint type; [FieldOffset(8)] public MI mi; [FieldOffset(8)] public KI ki; }
  [DllImport("user32.dll")] static extern uint SendInput(uint n, IN[] i, int size);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int h2, uint f);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  static void Send(IN x) { SendInput(1, new[] { x }, Marshal.SizeOf(typeof(IN))); }
  public static void Key(ushort vk, bool down) { var x = new IN { type = 1 }; x.ki.vk = vk; x.ki.flags = down ? 0u : 2u; Send(x); }
  public static void Combo(params ushort[] vks) { foreach (var v in vks) { Key(v, true); Thread.Sleep(30); } for (int i = vks.Length - 1; i >= 0; i--) { Key(vks[i], false); Thread.Sleep(30); } }
  public static void Type(string s) { foreach (var c in s) { foreach (var up in new[] { false, true }) { var x = new IN { type = 1 }; x.ki.scan = c; x.ki.flags = 4u | (up ? 2u : 0u); Send(x); Thread.Sleep(12); } } }
  public static void Move(int px, int py) { var x = new IN { type = 0 }; x.mi.dx = px * 65535 / (GetSystemMetrics(0) - 1); x.mi.dy = py * 65535 / (GetSystemMetrics(1) - 1); x.mi.flags = 0x8001; Send(x); }
  public static void Click(int px, int py, bool right) { Move(px, py); Thread.Sleep(150); var x = new IN { type = 0 }; x.mi.flags = right ? 8u : 2u; Send(x); Thread.Sleep(60); x.mi.flags = right ? 16u : 4u; Send(x); }
  public static void Wheel(int n, int delta, double hz) { var sw = System.Diagnostics.Stopwatch.StartNew(); for (int i = 0; i < n; i++) { var x = new IN { type = 0 }; x.mi.data = (uint)delta; x.mi.flags = 0x0800; Send(x); var due = (i + 1) * 1000.0 / hz; while (sw.Elapsed.TotalMilliseconds < due) Thread.Sleep(1); } }
  public static Bitmap Grab(int x, int y, int w, int h) { var b = new Bitmap(w, h); using (var g = Graphics.FromImage(b)) g.CopyFromScreen(x, y, 0, 0, new Size(w, h)); return b; }
}
"@
[W]::SetProcessDPIAware() | Out-Null   # physical pixels for window placement and screenshots
$VK = @{ ctrl = 0x11; shift = 0x10; alt = 0x12; ret = 0x0D; esc = 0x1B; f = 0x46; l = 0x4C; a = 0x41; p = 0x50; q = 0x51; k = 0x4B; c = 0x43; v = 0x56; n = 0x4E; del = 0x2E; z = 0x5A; h = 0x48; f2 = 0x71; space = 0x20; end = 0x23; comma = 0xBC }
function K([string[]]$names) { [W]::Combo([uint16[]]($names | ForEach-Object { $VK[$_] })); Start-Sleep -Milliseconds 350 }
function T($s) { [W]::Type($s); Start-Sleep -Milliseconds 350 }

$script:proc = $null; $script:hwnd = [IntPtr]::Zero; $script:log = $null
function Start-Atlas($arg, $logName, [hashtable]$envs) {
  $script:log = Join-Path $Out "$logName.log"
  foreach ($e in $envs.Keys) { Set-Item "Env:$e" $envs[$e] }
  $env:ATLAS_CONFIG_DIR = "$R\config-$Mode"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $script:proc = Start-Process -FilePath $Exe -ArgumentList "`"$arg`"" -PassThru -RedirectStandardError $script:log -RedirectStandardOutput "$script:log.out"
  return $sw
}
function Wait-Window {
  for ($i = 0; $i -lt 300; $i++) { $script:proc.Refresh(); if ($script:proc.MainWindowHandle -ne [IntPtr]::Zero) { $script:hwnd = $script:proc.MainWindowHandle; break }; Start-Sleep -Milliseconds 50 }
  [W]::ShowWindow($script:hwnd, 9) | Out-Null   # restore
  [W]::SetWindowPos($script:hwnd, [IntPtr]::Zero, 0, 0, 1366, 768, 0x0040) | Out-Null
  [W]::SetForegroundWindow($script:hwnd) | Out-Null
}
function Lines { if (Test-Path $script:log) { @(Get-Content $script:log -ErrorAction SilentlyContinue) } else { @() } }
# Wall-clock seconds from $sw start until the log first contains $pattern, then until it stops growing for $gap s.
function Burst($sw, [double]$t0, [double]$gap = 1.5, [int]$timeout = 60) {
  $len = -1; $last = [Math]::Max($t0, $sw.Elapsed.TotalSeconds)   # called late (cold start): count from now
  while ($sw.Elapsed.TotalSeconds - $last -lt $gap -and $sw.Elapsed.TotalSeconds -lt $t0 + $timeout) {
    $n = (Get-Item $script:log).Length
    if ($n -ne $len) { $len = $n; $last = $sw.Elapsed.TotalSeconds }
    Start-Sleep -Milliseconds 5
  }
  return $last - $t0
}
function First-Frame($sw) {
  while ($sw.Elapsed.TotalSeconds -lt 60) { if (Select-String -Path $script:log -Pattern '\[gpui-frame\]' -Quiet -ErrorAction SilentlyContinue) { return $sw.Elapsed.TotalSeconds }; Start-Sleep -Milliseconds 5 }
  return [double]::NaN
}
function Stop-Atlas {
  if ($script:proc -and -not $script:proc.HasExited) { [W]::SetForegroundWindow($script:hwnd) | Out-Null; K ctrl, q; if (-not $script:proc.WaitForExit(10000)) { Stop-Process -Id $script:proc.Id -Force } }
  Get-Process atlas-search-service -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$(Split-Path $Exe)*" } | Stop-Process -Force
  Start-Sleep 1
}
function Shot($name) { $r = New-Object W+RECT; [W]::GetWindowRect($script:hwnd, [ref]$r) | Out-Null; $b = [W]::Grab($r.L, $r.T, $r.R - $r.L, $r.B - $r.T); $b.Save((Join-Path $Out "$name.png")); $b.Dispose() }
function Focus { $r = New-Object W+RECT; [W]::GetWindowRect($script:hwnd, [ref]$r) | Out-Null; [W]::Click($r.L + 800, $r.T + 560, $false); Start-Sleep -Milliseconds 500; return $r }
function Goto($p) { K ctrl, l; Start-Sleep -Milliseconds 300; K ctrl, a; T $p; K ret }
# Like Goto, but returns a stopwatch started at the Enter press (for Burst).
function Goto-Timed($p) { K ctrl, l; Start-Sleep -Milliseconds 300; K ctrl, a; T $p; $sw = [Diagnostics.Stopwatch]::StartNew(); [W]::Combo([uint16[]]@($VK.ret)); return $sw }
# Seconds until a window-relative region stops changing for $stable s (like qmp.py settle).
function Settle($x0, $y0, $x1, $y1, $stable = 1.5, $timeout = 30) {
  $r = New-Object W+RECT; [W]::GetWindowRect($script:hwnd, [ref]$r) | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew(); $prev = $null; $changed = 0.0
  while ($sw.Elapsed.TotalSeconds -lt $timeout) {
    $now = $sw.Elapsed.TotalSeconds
    $b = [W]::Grab($r.L + $x0, $r.T + $y0, $x1 - $x0, $y1 - $y0); $ms = New-Object IO.MemoryStream; $b.Save($ms, [Drawing.Imaging.ImageFormat]::Bmp); $b.Dispose()
    $h = [BitConverter]::ToString([Security.Cryptography.MD5]::Create().ComputeHash($ms.ToArray()))
    if ($h -ne $prev) { $prev = $h; $changed = $now } elseif ($now - $changed -ge $stable) { return $changed }
    Start-Sleep -Milliseconds ([Math]::Max(0, 100 - ($sw.Elapsed.TotalSeconds - $now) * 1000))
  }
  return [double]::NaN
}
function RegionHash([int]$x0, [int]$y0, [int]$x1, [int]$y1) {   # window-relative; negative x1/y1 count from the right/bottom edge
  $r = New-Object W+RECT; [W]::GetWindowRect($script:hwnd, [ref]$r) | Out-Null
  if ($x1 -le 0) { $x1 += $r.R - $r.L }; if ($y1 -le 0) { $y1 += $r.B - $r.T }; if ($y0 -lt 0) { $y0 += $r.B - $r.T }; if ($x0 -lt 0) { $x0 += $r.R - $r.L }
  $b = [W]::Grab($r.L + $x0, $r.T + $y0, $x1 - $x0, $y1 - $y0); $ms = New-Object IO.MemoryStream; $b.Save($ms, [Drawing.Imaging.ImageFormat]::Bmp); $b.Dispose()
  [BitConverter]::ToString([Security.Cryptography.MD5]::Create().ComputeHash($ms.ToArray()))
}
# Opens $p and times it from the Enter press by screen polling (Atlas renders only on demand, so the frame
# log is no end signal): count_s = when the status-bar item count first changes, settled_s = last change of
# list + status bar before 1.5 s without change (after the count changed). Shots at 1, 3, 10 s.
function Open-Timed($p, $name, $timeout = 90) {
  $count = { RegionHash -260 -50 -40 -16 }; $view = { RegionHash 250 90 -10 -16 }
  K ctrl, l; Start-Sleep -Milliseconds 300; K ctrl, a; T $p
  $c0 = & $count; $sw = [Diagnostics.Stopwatch]::StartNew(); [W]::Combo([uint16[]]@($VK.ret))
  $tc = [double]::NaN; $prev = $null; $last = 0.0; $shots = @(1, 3, 10)
  while ($sw.Elapsed.TotalSeconds -lt $timeout) {
    $now = $sw.Elapsed.TotalSeconds
    if ($shots.Count -and $now -ge $shots[0]) { Shot ("$name-{0}s" -f $shots[0]); $shots = @($shots | Select-Object -Skip 1) }
    if ([double]::IsNaN($tc) -and (& $count) -ne $c0) { $tc = $now }
    $h = & $view; if ($h -ne $prev) { $prev = $h; $last = $now } elseif (-not [double]::IsNaN($tc) -and $now - $last -ge 1.5) { break }
    Start-Sleep -Milliseconds 30
  }
  "count_s={0:N3} settled_s={1:N3}" -f $tc, $last
}
function Idle {
  Start-Sleep 5; $p = Get-Process -Id $script:proc.Id; $c0 = $p.TotalProcessorTime.TotalSeconds; Start-Sleep 30; $p.Refresh()
  $cpu = ($p.TotalProcessorTime.TotalSeconds - $c0) / 30 * 100   # % of one core, as on Linux
  $pws = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($p.Id)").WorkingSetPrivate
  "idle_cpu_pct={0:N2} ws_mib={1} private_ws_mib={2}" -f $cpu, [int]($p.WorkingSet64 / 1MB), [int]($pws / 1MB)
}

$big = "$R\perf\big"; $small = "$R\perf\small"
if (-not (Test-Path "$big\f100000")) { New-Item -ItemType Directory -Force $big, $small | Out-Null; 1..100000 | ForEach-Object { [IO.File]::Create(("{0}\f{1:D6}" -f $big, $_)).Dispose() }; 1..40 | ForEach-Object { Set-Content "$small\file-$_.txt" $_ } }
$cpuName = (Get-CimInstance Win32_Processor).Name; $gpu = (Get-CimInstance Win32_VideoController).Name
Set-Content (Join-Path $Out 'system.txt') "$cpuName | $gpu | $((Get-CimInstance Win32_OperatingSystem).Caption) | screen $([W]::GetSystemMetrics(0))x$([W]::GetSystemMetrics(1)) | $Exe"

# perf is driven from the Linux host by win-perf.sh: setup, cold (x3, standby list purged before each), main.
if ($Mode -eq 'setup') {
  Remove-Item -Recurse -Force "$R\config-perf", "$R\config-frames" -ErrorAction SilentlyContinue
  foreach ($m in 'perf', 'frames') { $env:ATLAS_CONFIG_DIR = "$R\config-$m"; $script:log = "$Out\setup-$m.log"
    $script:proc = Start-Process -FilePath $Exe -ArgumentList "`"$R\perf`"" -PassThru -RedirectStandardError $script:log -RedirectStandardOutput "$script:log.out"
    Wait-Window; Start-Sleep 8; K esc; Start-Sleep 2; Stop-Atlas }
}
if ($Mode -like 'cold*') {
  $Mode = 'perf'; $i = (Get-ChildItem $Out -Filter 'cold*.log' | Measure-Object).Count + 1
  $sw = Start-Atlas "$R\perf" "cold$i" @{ GPUI_FRAME_LOG = '0' }; $ff = First-Frame $sw; Wait-Window; $st = Burst $sw 0
  Log ("MEASURE cold$i first_frame_s={0:N3} settled_s={1:N3}" -f $ff, $st); Shot "cold$i"; Stop-Atlas
}
if ($Mode -eq 'main') {
  $Mode = 'perf'
  $null = Start-Atlas "$R\perf" 'main' @{ GPUI_FRAME_LOG = '0' }; Wait-Window; Start-Sleep 5; Log "MEASURE $(Idle)"
  # $wr, not $r: PowerShell names are case-insensitive and $R is the run directory.
  $wr = Focus
  # win-perf.sh purges the standby list (admin) between need-purge and purged, like drop_caches on Linux.
  # (win-perf.sh deletes both flags; the console user may not delete files the ssh account created.)
  Set-Content "$R\need-purge.flag" 1; while (-not (Test-Path "$R\purged.flag")) { Start-Sleep -Milliseconds 200 }; Start-Sleep 2
  [W]::SetForegroundWindow($script:hwnd) | Out-Null
  Log "MEASURE big_cold $(Open-Timed $big 'big-cold')"; Shot 'big-cold'
  Goto $small; Start-Sleep 4
  Log "MEASURE big_warm $(Open-Timed $big 'big-warm')"; Shot 'big-warm'
  [W]::Move($wr.L + 800, $wr.T + 400); Start-Sleep 1
  $a = (Lines).Count; $p = Get-Process -Id $script:proc.Id; $c0 = $p.TotalProcessorTime.TotalSeconds
  $sw = [Diagnostics.Stopwatch]::StartNew(); [W]::Wheel(180, -120, 60); $dur = $sw.Elapsed.TotalSeconds; Start-Sleep 2; $p.Refresh()
  $all = Lines; $frames = @($all[$a..($all.Count - 1)] | Where-Object { $_ -match '\[gpui-frame\]' }).Count
  Log ("MEASURE scroll wheel_s={0:N2} fps={1:N1} cpu_s={2:N2}" -f $dur, ($frames / $dur), ($p.TotalProcessorTime.TotalSeconds - $c0)); Shot 'scrolled'
  Goto "$R\perf"; Start-Sleep 3; $null = Focus
  # Ctrl+F opens the filter box at the bottom left; its subfolder toggle (Expanded Search) searches the
  # index of atlas-search-service. Type the query first: with an empty query the toggle lists all 100k
  # files under perf\ and blocks the UI for ~45 s. Timed from the toggle click until the list settles.
  $wr = New-Object W+RECT; [W]::GetWindowRect($script:hwnd, [ref]$wr) | Out-Null
  K ctrl, f; Start-Sleep 1; [W]::Click($wr.L + 330, $wr.B - 38, $false); Start-Sleep -Milliseconds 400
  T 'f099999'; Start-Sleep 2; Shot 'search-box'; [W]::Click($wr.L + 468, $wr.B - 38, $false)
  Log ("MEASURE search settle_s={0:N2}" -f (Settle 10 60 1340 700)); Shot 'search'; K esc
  Stop-Atlas
}
if ($Mode -eq 'frames') {
  # First run with a fresh profile: dismiss the welcome assistant once (as frames.sh does).
  Remove-Item -Recurse -Force "$R\config-$Mode" -ErrorAction SilentlyContinue
  $null = Start-Atlas "$R\perf" 'frames-setup' @{}; Wait-Window; Start-Sleep 8; K esc; Start-Sleep 2; Stop-Atlas
  $envs = @{ ATLAS_PERF = '1'; ATLAS_PERF_EVENTS = '1'; ATLAS_PERF_MS = '4'; GPUI_FRAME_LOG = '0'; RUST_LOG = 'info' }
  $null = Start-Atlas $big 'frames' $envs; Wait-Window; Start-Sleep 10; $wr = Focus
  [W]::Move($wr.L + 800, $wr.T + 400); Start-Sleep 1
  $a = (Lines).Count; [W]::Wheel(180, -120, 60); [W]::Wheel(180, 120, 60); Start-Sleep 1; Add-Content "$Out\marks.txt" "scroll $a $((Lines).Count)"; Shot 'scroll'
  $a = (Lines).Count
  foreach ($i in 1..10) { $sz = if ($i % 2) { @(1000, 600) } else { @(1366, 768) }; [W]::SetWindowPos($script:hwnd, [IntPtr]::Zero, 0, 0, $sz[0], $sz[1], 0x0040) | Out-Null; Start-Sleep -Milliseconds 1200 }
  Start-Sleep 1; Add-Content "$Out\marks.txt" "resize $a $((Lines).Count)"; Shot 'resize'
  $null = Focus; $a = (Lines).Count
  foreach ($round in 1, 2) { foreach ($v in 'Thumbnails', 'List', 'Columns', 'Tree', 'Details') { K ctrl, shift, p; Start-Sleep -Milliseconds 800; T "View: $v"; Start-Sleep -Milliseconds 800; K ret; Start-Sleep -Milliseconds 1500; Shot "view-$v" } }
  Start-Sleep 1; Add-Content "$Out\marks.txt" "views $a $((Lines).Count)"
  Stop-Atlas
}
Log "DONE $Mode"
