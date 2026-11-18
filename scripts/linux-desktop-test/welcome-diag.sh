#!/usr/bin/env bash
# Idle cost of Atlas's first-run welcome screen: frames and CPU over 20 s in four states.
# Per state: atlas CPU %, GPUI frames/s and app frames/s from the perf log, the busiest threads (top -H), a screenshot.
# On GNOME close the login overview first (dvm.sh DISTRO qmp key Escape).
#   welcome-diag.sh DISTRO BIN OUT_DIR       (BIN = a perf-log atlas inside the VM, e.g. atlas or ~/atlas-perf/atlas)
# The VM must be up with a desktop session; nothing else heavy should run on the host meanwhile.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
d=${1:?distro} bin=${2:?atlas path in the VM} out=${3:?out dir}; mkdir -p "$out"; : > "$out/results.txt"
vm() { "$here/dvm.sh" "$d" ssh "$@"; }
q() { "$here/dvm.sh" "$d" qmp "$@"; }
log() { printf '%s\n' "$*" | tee -a "$out/results.txt"; }
cfg='~/.config/atlas-diag'
# Guest command: stop atlas-search-service, matched by the file name of argv[0] (any path, spaces too), so it
# never matches the shell that runs it or another program whose arguments mention it.
kill_search='for p in /proc/[0-9]*; do a=$(tr "\0" "\n" < "$p/cmdline" 2>/dev/null | head -n1); [ "${a##*/}" = atlas-search-service ] && kill "${p#/proc/}" 2>/dev/null; done; true'
measure() { # $1 = label: frames, frame_summary lines, CPU % of atlas over 20 s
  r=$(vm 'p=$(pgrep -x atlas); hz=$(getconf CLK_TCK); l0=$(wc -l < ~/diag.log); a=$(awk "{print \$14+\$15}" /proc/$p/stat); sleep 20;
    b=$(awk "{print \$14+\$15}" /proc/$p/stat); l1=$(wc -l < ~/diag.log);
    f=$(sed -n "$((l0+1)),${l1}p" ~/diag.log | grep -c "gpui-frame\] total");
    s=$(sed -n "$((l0+1)),${l1}p" ~/diag.log | grep -c frame_summary);
    awk -v d=$((b-a)) -v hz=$hz -v f=$f -v s=$s "BEGIN{printf \"cpu_pct=%.1f gpui_frames_per_s=%.1f app_frames_per_s=%.1f\", d/hz/20*100, f/20, s/20}";
    echo " lines=$l0-$l1"')
  log "MEASURE $1 $r"
  vm 'top -H -b -d 5 -n 2 -p $(pgrep -x atlas) | awk "/^top -/{n++} n==2" | sed -n "7,14p"' | tee -a "$out/threads-$1.txt"
  q shot "$out/$1.png"
}
vm 'pkill -x atlas; '"$kill_search"'; sleep 1; true'
vm "$bin --version | head -1; (. /etc/os-release; echo \$PRETTY_NAME); echo desktop=\$XDG_CURRENT_DESKTOP; ls /run/user/\$(id -u) | grep -E 'sway|wayland'" | tee "$out/system.txt"
vm "rm -rf $cfg; mkdir -p ~/perf; ATLAS_PERF=1 ATLAS_PERF_EVENTS=1 ATLAS_PERF_MS=0 GPUI_FRAME_LOG=0 RUST_LOG=info \
  ATLAS_CONFIG_DIR=$cfg nohup ~/insession $bin ~/perf > ~/diag.log 2>&1 &"
sleep 25
measure 1-welcome-input-focused
q click 120 420; sleep 3              # dark background of the welcome page: focus leaves the profile field
measure 2-welcome-clicked-background
q key Escape; sleep 4                 # close the welcome screen: plain file browser
measure 3-browser-idle
q click 800 560; sleep 0.5; q key ctrl-shift-f; sleep 2   # search field focused (caret); click first so the window has focus
measure 4-browser-search-focused
q key Escape; sleep 1
"$here/dvm.sh" "$d" get /home/atlas/diag.log "$out/diag.log"
vm 'pkill -x atlas; '"$kill_search"'; true'
