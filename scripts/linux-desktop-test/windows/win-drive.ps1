# Interactive driver for the Windows feature checks: runs in the console user's session (task.ps1)
# and executes every new $R\cmd\NNN.ps1 (only Administrators can write there) in this runspace,
# writing $R\drv\NNN.out. Stops on $R\cmd\stop or after 20 minutes without a command.
$ErrorActionPreference = 'Continue'
$R = 'C:\Users\Public\atlas-cmp-run'; $D = "$R\drv"; New-Item -ItemType Directory -Force $D | Out-Null
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
  public static void Button(bool right, bool down) { var x = new IN { type = 0 }; x.mi.flags = right ? (down ? 8u : 16u) : (down ? 2u : 4u); Send(x); }
  // Atlas starts an OLE drag only after a slow first move inside its own window.
  public static void Drag(int x1, int y1, int x2, int y2) { Move(x1, y1); Thread.Sleep(300); Button(false, true); Thread.Sleep(300); for (int i = 1; i <= 20; i++) { Move(x1 + i * 3, y1 + i * 2); Thread.Sleep(30); } Thread.Sleep(500); int sx = x1 + 60, sy = y1 + 40; for (int i = 1; i <= 60; i++) { Move(sx + (x2 - sx) * i / 60, sy + (y2 - sy) * i / 60); Thread.Sleep(40); } Thread.Sleep(1000); Move(x2 + 5, y2 + 5); Thread.Sleep(1000); Button(false, false); }
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public static Bitmap Grab(int x, int y, int w, int h) { var b = new Bitmap(w, h); using (var g = Graphics.FromImage(b)) g.CopyFromScreen(x, y, 0, 0, new Size(w, h)); return b; }
}
"@
[W]::SetProcessDPIAware() | Out-Null   # physical pixels for window placement and screenshots
$VK = @{ ctrl = 0x11; shift = 0x10; alt = 0x12; ret = 0x0D; esc = 0x1B; f = 0x46; l = 0x4C; a = 0x41; p = 0x50; q = 0x51; k = 0x4B; c = 0x43; v = 0x56; n = 0x4E; del = 0x2E; z = 0x5A; h = 0x48; f2 = 0x71; space = 0x20; end = 0x23; comma = 0xBC }
[W]::SetProcessDPIAware() | Out-Null
function K([string[]]$names) { [W]::Combo([uint16[]]($names | ForEach-Object { $VK[$_] })); Start-Sleep -Milliseconds 350 }
function T($s) { [W]::Type($s); Start-Sleep -Milliseconds 350 }


function Rect($h) { $r = New-Object W+RECT; [W]::GetWindowRect($h, [ref]$r) | Out-Null; $r }
function Place($h, $x, $y, $w, $hh) { [W]::ShowWindow($h, 9) | Out-Null; [W]::SetWindowPos($h, [IntPtr]::Zero, $x, $y, $w, $hh, 0x0040) | Out-Null; [W]::SetForegroundWindow($h) | Out-Null; Start-Sleep -Milliseconds 400 }
# Screenshot of a screen region (default: the region of window $h) into $D\name.png.
function Shot($name, $h, $x, $y, $w, $hh) { if ($h) { $r = Rect $h; $x = $r.L; $y = $r.T; $w = $r.R - $r.L; $hh = $r.B - $r.T }; $b = [W]::Grab($x, $y, $w, $hh); $b.Save("$D\$name.png"); $b.Dispose() }
$script:atlas = $null
function Atlas-Start($exe, $arg, $cfg) {
  $env:ATLAS_CONFIG_DIR = $cfg
  $script:atlas = Start-Process -FilePath $exe -ArgumentList "`"$arg`"" -PassThru -RedirectStandardError "$D\atlas.log" -RedirectStandardOutput "$D\atlas.out.log"
  for ($i = 0; $i -lt 300; $i++) { $script:atlas.Refresh(); if ($script:atlas.MainWindowHandle -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 50 }
  $script:atlas.MainWindowHandle
}
function Goto($p) { K ctrl, l; Start-Sleep -Milliseconds 300; K ctrl, a; T $p; K ret }
function AH { $script:atlas.Refresh(); $script:atlas.MainWindowHandle }
# Explorer window showing $path (opens one if needed); returns its HWND.
function Explorer($path) {
  $sh = New-Object -ComObject Shell.Application
  $u = ([Uri]$path).AbsoluteUri
  $w = $sh.Windows() | Where-Object { $_.LocationURL -eq $u } | Select-Object -First 1
  if (-not $w) { Start-Process explorer.exe "`"$path`""; for ($i = 0; $i -lt 50 -and -not $w; $i++) { Start-Sleep -Milliseconds 200; $w = $sh.Windows() | Where-Object { $_.LocationURL -eq $u } | Select-Object -First 1 } }
  if ($w) { [IntPtr]$w.HWND } else { [IntPtr]::Zero }
}
function Explorer-Close($path) { $u = ([Uri]$path).AbsoluteUri; (New-Object -ComObject Shell.Application).Windows() | Where-Object { $_.LocationURL -eq $u } | ForEach-Object { $_.Quit() } }

$last = Get-Date
while (((Get-Date) - $last).TotalMinutes -lt 20 -and -not (Test-Path "$R\cmd\stop")) {
  foreach ($f in Get-ChildItem "$R\cmd" -Filter '*.ps1' | Where-Object { $_.BaseName -match '^\d+$' } | Sort-Object Name) {
    $o = "$D\$($f.BaseName).out"; if (Test-Path $o) { continue }
    $last = Get-Date
    try { $res = . $f.FullName 2>&1 | Out-String -Width 300 } catch { $res = "ERROR: $_" }
    Set-Content $o ($res + "`n#done") -Encoding utf8
  }
  Start-Sleep -Milliseconds 300
}
