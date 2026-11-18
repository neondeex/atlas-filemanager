#!/usr/bin/env bash
# Send one PowerShell snippet to win-drive.ps1 on a Windows PC and print its output (waits for it).
# ATLAS_WIN_HOST is its OpenSSH host (see win-perf.sh).
#   win-do.sh start        start the driver in the console session (checks quser idle >= 15 min)
#   win-do.sh 'snippet'    run a snippet;   win-do.sh stop    stop the driver
#   win-do.sh get NAME DIR copy $R\drv\NAME.png to DIR
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); host=${ATLAS_WIN_HOST:?set ATLAS_WIN_HOST}; R='C:\Users\Public\atlas-cmp-run'; RF=C:/Users/Public/atlas-cmp-run
w() { ssh "$host" "$@"; }
# shellcheck source=lib.sh
. "$here/lib.sh"
case ${1:-} in
start)
  line=$( (w quser 2>/dev/null || true) | quser_console)
  [[ -n $line ]] || { echo "nobody on the console of $host" >&2; exit 3; }
  user=${line% *} idle=${line##* }
  (( $(idle_minutes "$idle") >= 15 )) || { echo "$host in use (idle $idle)" >&2; exit 3; }
  w "(if not exist $R\\cmd mkdir $R\\cmd) & del /q $R\\cmd\\* 2>nul & (if exist $R\\drv rmdir /s /q $R\\drv)"
  w "icacls $R\\cmd /inheritance:r /grant:r Administrators:(OI)(CI)F SYSTEM:(OI)(CI)F \"$user:(OI)(CI)RX\" >nul"
  scp -q "$here/win-drive.ps1" "$here/win-task.ps1" "$host:$RF/cmd/"; w "move /y $R\\cmd\\win-task.ps1 $R\\task.ps1 >nul"
  w "powershell -NoProfile -ExecutionPolicy Bypass -File $R\\task.ps1 -User \"$user\" -Name AtlasDrive -Args2 \"-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File $R\\cmd\\win-drive.ps1\""
  echo "driver started as $user" ;;
stop) w "echo 1 > $R\\cmd\\stop"; sleep 2; w 'powershell -NoProfile -Command "Unregister-ScheduledTask AtlasDrive -Confirm:$false"' ;;
get) scp -q "$host:$RF/drv/$2.png" "$3/" ;;
*)
  n=$(date +%s%N | cut -c1-13); f=$(mktemp); printf '%s\n' "$1" > "$f"; scp -q "$f" "$host:$RF/cmd/$n.ps1"; rm -f "$f"
  for _ in $(seq 1 "${WAIT:-120}"); do
    if out=$(w "type $R\\drv\\$n.out" 2>/dev/null) && [[ $out == *'#done'* ]]; then printf '%s\n' "${out%#done*}"; exit 0; fi
    sleep 1
  done
  echo "timeout waiting for $n" >&2; exit 2 ;;
esac
