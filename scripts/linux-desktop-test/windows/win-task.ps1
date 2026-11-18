# Register + start a scheduled task in the console user's interactive session (no password needed).
#   task.ps1 -User <console user> -Args2 "<arguments for powershell.exe>"
param([string]$User, [string]$Args2, [string]$Name = 'AtlasBench', [int]$Wait = 1)
$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $Args2
$p = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Limited
$s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 2)
Register-ScheduledTask -TaskName $Name -Action $a -Principal $p -Settings $s -Force | Out-Null
Start-ScheduledTask -TaskName $Name
