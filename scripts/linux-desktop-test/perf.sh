#!/usr/bin/env bash
# Performance of the installed Atlas in a desktop VM (run on the host):
#   perf.sh DISTRO [OUT_DIR]      (default $ATLAS_TEST_OUT/perf-DISTRO, ATLAS_TEST_OUT=~/atlas-desktop-test)
# Start, folder and scroll timings come from Atlas's own WAYLAND_DEBUG log inside the guest (first
# buffer attach, keyboard events, surface commits), so they mean the same thing on every desktop.
# Idle CPU and memory come from /proc in the guest; search is timed from screenshots (step 5).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
d=${1:?distro}; out=${2:-${ATLAS_TEST_OUT:-$HOME/atlas-desktop-test}/perf-$d}
mkdir -p "$out"; : > "$out/results.txt"
vm() { "$here/dvm.sh" "$d" ssh "$@"; }
q() { "$here/dvm.sh" "$d" qmp "$@"; }
log() { printf '%s\n' "$*" | tee -a "$out/results.txt"; }
cfg='~/.config/atlas-perf'
# Click into the file list so key presses go to Atlas (FOCUS_X/FOCUS_Y for a smaller window).
focus() { q click "${FOCUS_X:-800}" "${FOCUS_Y:-560}"; sleep 0.5; }
# Guest command: stop atlas-search-service, matched by the file name of argv[0] (any path, spaces too), so it
# never matches the shell that runs it or another program whose arguments mention it.
kill_search='for p in /proc/[0-9]*; do a=$(tr "\0" "\n" < "$p/cmdline" 2>/dev/null | head -n1); [ "${a##*/}" = atlas-search-service ] && kill "${p#/proc/}" 2>/dev/null; done; true'
stop() { vm 'pkill -x atlas; '"$kill_search"'; for i in $(seq 40); do pgrep -x atlas >/dev/null || exit 0; sleep 0.25; done; pkill -9 -x atlas; true'; sleep 1; }
launch() { # $1 = log name; prints the guest epoch at launch
  vm "mkdir -p ~/perf-logs; s=\$(date +%s.%N); WAYLAND_DEBUG=1 ATLAS_CONFIG_DIR=$cfg nohup ~/insession atlas ~/perf > ~/perf-logs/$1.log 2>&1 & echo \$s"
}
# wdlog.py on a fetched log; stops the run when it prints nothing (a missing log or a changed log format).
wd() { local r; r=$(python3 "$here/wdlog.py" "$@") || true; [[ -n $r ]] || { echo "wdlog.py $* gave no result" >&2; return 1; }; printf '%s\n' "$r"; }
fetch() { "$here/dvm.sh" "$d" get "/home/atlas/perf-logs/$1.log" "$out/$1.log"; }
goto() { q key ctrl-l; sleep 0.6; q key ctrl-a; q type "$1"; q key Return; }

vm "mkdir -p ~/perf/big ~/perf/small && cd ~/perf/big && [ \$(ls | wc -l) -eq 100000 ] || (seq -w 1 100000 | sed 's/^/f/' | xargs touch); for i in \$(seq 1 40); do echo \$i > ~/perf/small/file-\$i.txt; done; uname -r; (. /etc/os-release; echo \$PRETTY_NAME); atlas --version 2>/dev/null | head -1" | tee "$out/system.txt"

# GNOME opens the Activities overview after login; close it, or the Escape below never reaches Atlas and
# every measurement runs on the welcome assistant (harmless on sway).
q key Escape; sleep 1
# First run: dismiss the welcome assistant once so the measured starts show the browser.
stop; vm "rm -rf $cfg"; launch setup >/dev/null; sleep 8; q key Escape; sleep 2; stop

# 1. Cold start (page cache dropped) x3
for i in 1 2 3; do
  vm 'sync; echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null'; sleep 2
  s=$(launch cold$i); sleep 10; q shot "$out/cold$i.png"
  if [[ $i == 3 ]]; then
    # 2. Idle cost after start (30 s), with the window shown and nothing happening
    sleep 5
    vm 'p=$(pgrep -x atlas); hz=$(getconf CLK_TCK); a=$(awk "{print \$14+\$15}" /proc/$p/stat); sleep 30; b=$(awk "{print \$14+\$15}" /proc/$p/stat);
        awk -v d=$((b-a)) -v hz=$hz "BEGIN{printf \"idle_cpu_pct=%.2f \", d/hz/30*100}";
        awk "/^Rss:/{r=\$2} /^Pss:/{p=\$2} END{printf \"rss_mib=%d pss_mib=%d\n\", r/1024, p/1024}" /proc/$p/smaps_rollup' | sed 's/^/MEASURE /' | tee -a "$out/results.txt"
  else
    stop
  fi
  fetch cold$i; m=$(wd "$out/cold$i.log" first-frame "$s"); log "MEASURE cold$i $m"
done

# 3. Open the 100 000-file folder (cold page cache), then again (warm)
focus
vm 'sync; echo 3 | sudo tee /proc/sys/vm/drop_caches >/dev/null'; sleep 2
goto /home/atlas/perf/big; sleep 12; q shot "$out/big-cold.png"
goto /home/atlas/perf/small; sleep 4
goto /home/atlas/perf/big; sleep 10; q shot "$out/big-warm.png"
fetch cold3
m=$(wd "$out/cold3.log" after-key 28 3); log "MEASURE big_cold $m"
m=$(wd "$out/cold3.log" after-key 28 1); log "MEASURE big_warm $m"

# 4. Scroll the 100 000-file listing: 180 wheel notches at 60 Hz
q move 800 400; sleep 1
cpu0=$(vm 'awk "{print \$14+\$15}" /proc/$(pgrep -x atlas)/stat')
q wheel 180 down 60; sleep 2
cpu1=$(vm 'awk "{print \$14+\$15}" /proc/$(pgrep -x atlas)/stat')
q shot "$out/scrolled.png"
fetch cold3
m=$(wd "$out/cold3.log" scroll); log "MEASURE scroll $m cpu_s=$(awk -v a="$cpu0" -v b="$cpu1" 'BEGIN{printf "%.2f", (b-a)/100}')"

# 5. Search in subfolders of ~/perf for one name among 100 040 entries
goto /home/atlas/perf; sleep 3; focus
# The search field's blinking cursor redraws all the time, so this one is timed from screenshots
# of the result list: from the last typed key until the list stops changing.
# The list before typing is saved, and the time counts until the list differs from it and stops
# changing, so a list that never changes is a failure, not a 0 s search. It does not check that the
# list shows f099999: search.png is kept for that.
q key ctrl-shift-f; sleep 1
base=$(mktemp); trap 'rm -f "$base"' EXIT
q region 240 110 1340 700 "$base"
q type f099999; m=$(q settle 240 110 1340 700 1.5 30 "$base")
[[ $m =~ ^settle_s=[0-9] ]] || { echo "search gave no new result list within 30 s: ${m:-nothing}" >&2; exit 1; }
log "MEASURE search $m"; q shot "$out/search.png"; q key Escape
stop
