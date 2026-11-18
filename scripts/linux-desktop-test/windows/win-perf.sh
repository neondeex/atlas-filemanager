#!/usr/bin/env bash
# Runs win-bench.ps1 on a Windows PC inside the logged-on desktop session (a scheduled task with an
# interactive principal for the console user), never in SSH session 0. Stops if someone used the PC
# in the last 15 minutes. The timed modes (perf, frames) also stop if rustc is running there.
#   ATLAS_WIN_HOST=<ssh host> win-perf.sh perf|frames|setup EXE_DIR OUT_LOCAL
# ATLAS_WIN_HOST is an OpenSSH host (Windows OpenSSH server, an admin account, cmd.exe as its shell).
# EXE_DIR is a folder on the PC with atlas.exe and atlas-search-service.exe. The console user may not
# be able to read the ssh account's profile, so EXE_DIR is copied to C:\Users\Public\atlas-cmp-run\bin-<mode>
# first. Clean up afterwards with: rmdir /s /q C:\Users\Public\atlas-cmp-run
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
what=${1:?mode} src=${2:?exe dir} out=${3:?local out dir}; host=${ATLAS_WIN_HOST:?set ATLAS_WIN_HOST}
case $what in perf|frames|setup) ;; *) echo "unknown mode $what (perf, frames or setup)" >&2; exit 2 ;; esac
R='C:\Users\Public\atlas-cmp-run'; bin="$R\\bin-$what"
w() { ssh "$host" "$@"; }
# shellcheck source=lib.sh
. "$here/lib.sh"
console_user=
idle_ok() {  # console user from quser; needs >= 15 min idle
  local line; line=$( (w quser 2>/dev/null || true) | quser_console)
  [[ -n $line ]] || { echo "nobody on the console of $host" >&2; return 1; }
  console_user=${line% *}; local idle=${line##* }
  (( $(idle_minutes "$idle") >= 15 ))
}
# No rustc.exe running; a failed tasklist counts as a build, so the run does not start blind.
no_build() { local t; t=$(w 'tasklist /FI "IMAGENAME eq rustc.exe" /NH') || return 1; ! grep -qi rustc.exe <<<"$t"; }
run() {  # $1 = mode; waits until the task finished
  idle_ok || { echo "someone is using $host (or nobody is logged on); try later" >&2; exit 3; }
  [[ $1 == setup ]] || no_build || { echo "a cargo build runs on $host; try later" >&2; exit 4; }
  w "powershell -NoProfile -ExecutionPolicy Bypass -File $R\\task.ps1 -User \"$console_user\" -Args2 \"-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File $R\\win-bench.ps1 -Mode $1 -Exe $bin\\atlas.exe -Out $R\\out-$what\"" >/dev/null
  sleep 3
  while w 'powershell -NoProfile -Command "(Get-ScheduledTask AtlasBench).State"' | grep -q Running; do
    if w "if exist $R\\need-purge.flag (del $R\\need-purge.flag & exit 0) else (exit 1)" 2>/dev/null; then
      w "powershell -NoProfile -ExecutionPolicy Bypass -File $R\\purge-standby.ps1"; w "echo 1 > $R\\purged.flag"
    fi
    sleep 2
  done
  w "if exist $R\\purged.flag del $R\\purged.flag"
}
# Check before anything is copied to the PC.
idle_ok || { echo "someone is using $host (or nobody is logged on); try later" >&2; exit 3; }
[[ $what == setup ]] || no_build || { echo "a cargo build runs on $host; try later" >&2; exit 4; }
w "if not exist $R mkdir $R"
scp -q "$here/win-bench.ps1" "$here/purge-standby.ps1" "$here/win-task.ps1" "$host:${R//\\//}/"
w "move /y $R\\win-task.ps1 $R\\task.ps1 >nul"
w "robocopy \"$src\" \"$bin\" atlas.exe atlas-search-service.exe /NJH /NJS /NFL /NDL >nul & (if exist $R\\out-$what rmdir /s /q $R\\out-$what) & del /q $R\\*.flag 2>nul"
case $what in
  perf)
    run setup
    for _ in 1 2 3; do w "powershell -NoProfile -ExecutionPolicy Bypass -File $R\\purge-standby.ps1"; sleep 2; run cold; done
    run main ;;
  *) run "$what" ;;
esac
w 'powershell -NoProfile -Command "Unregister-ScheduledTask AtlasBench -Confirm:$false"' >/dev/null
mkdir -p "$out"; scp -q -r "$host:${R//\\//}/out-$what/*" "$out/"
cat "$out/results.txt" 2>/dev/null || true
