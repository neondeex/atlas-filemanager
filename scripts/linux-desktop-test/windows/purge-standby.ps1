# Empties the Windows standby list (file cache), the counterpart of drop_caches on Linux. Needs admin.
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class P {
  [DllImport("ntdll.dll")] public static extern uint NtSetSystemInformation(int c, ref int v, int l);
  [DllImport("ntdll.dll")] public static extern uint RtlAdjustPrivilege(int p, bool e, bool t, out bool was);
}
"@
$was = $false; [void][P]::RtlAdjustPrivilege(13, $true, $false, [ref]$was)   # SeProfileSingleProcessPrivilege
$cmd = 4; $st = [P]::NtSetSystemInformation(80, [ref]$cmd, 4)                 # MemoryPurgeStandbyList
"purge status=0x{0:X}" -f $st
