#!/usr/bin/env bash
# Frame times of the perf-log test build in a desktop VM (run on the host):
#   frames.sh DISTRO [OUT_DIR]    (default $ATLAS_TEST_OUT/frames-DISTRO, ATLAS_TEST_OUT=~/atlas-desktop-test)
# Same switches as upstream's scripts/run-release-perf.ps1: ATLAS_PERF=1, ATLAS_PERF_EVENTS=1,
# ATLAS_PERF_MS=4, GPUI_FRAME_LOG=0 (every frame logged). The binary is ~/atlas-perf/atlas
# (ATLAS_BIN overrides it, e.g. a Nix store path).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
d=${1:?distro}; out=${2:-${ATLAS_TEST_OUT:-$HOME/atlas-desktop-test}/frames-$d}
mkdir -p "$out"; : > "$out/results.txt"
vm() { "$here/dvm.sh" "$d" ssh "$@"; }
q() { "$here/dvm.sh" "$d" qmp "$@"; }
log() { printf '%s\n' "$*" | tee -a "$out/results.txt"; }
bin=${ATLAS_BIN:-'~/atlas-perf/atlas'}
cfg='~/.config/atlas-frames'
mark() { vm 'wc -l < ~/frames.log'; }
focus() { q click "${FOCUS_X:-800}" "${FOCUS_Y:-560}"; sleep 0.5; }
# Guest command: stop atlas-search-service, matched by the file name of argv[0] (any path, spaces too), so it
# never matches the shell that runs it or another program whose arguments mention it.
kill_search='for p in /proc/[0-9]*; do a=$(tr "\0" "\n" < "$p/cmdline" 2>/dev/null | head -n1); [ "${a##*/}" = atlas-search-service ] && kill "${p#/proc/}" 2>/dev/null; done; true'
stop() { vm 'pkill -x atlas; '"$kill_search"'; sleep 1; pkill -9 -x atlas; true'; sleep 1; }
start() {
  vm "ATLAS_PERF=1 ATLAS_PERF_EVENTS=1 ATLAS_PERF_MS=4 GPUI_FRAME_LOG=0 RUST_LOG=info ATLAS_CONFIG_DIR=$cfg \
    nohup ~/insession $bin $1 > ~/frames.log 2>&1 &"
  sleep 10
}
# frames.py over the log lines [$1, $2); stops the run when the log cannot be read, or it has no draw or gap.
fr() {
  "$here/dvm.sh" "$d" ssh 'cat ~/frames.log' > "$out/frames.part" || { echo "cannot read the guest's frames.log" >&2; return 1; }
  local r; r=$(python3 "$here/frames.py" "$out/frames.part" "$1" "$2") || true
  [[ $r == *'draw: n='[1-9]*'gap: n='[1-9]* ]] || { echo "frames.py $1 $2 gave no frames: ${r:-nothing}" >&2; return 1; }
  printf '%s\n' "$r"
}
# A view by its palette command title ("View: Thumbnails" etc.).
view() { q key ctrl-shift-p; sleep 0.8; q type "$1"; sleep 0.8; q key Return; sleep 1.5; }

vm "mkdir -p ~/perf/big; cd ~/perf/big; [ \$(ls | wc -l) -eq 100000 ] || (seq -w 1 100000 | sed 's/^/f/' | xargs touch)
  $bin --version | head -1; (. /etc/os-release; echo \$PRETTY_NAME)
  echo \"renderer: \$(ls /usr/share/vulkan/icd.d /run/opengl-driver/share/vulkan/icd.d 2>/dev/null | grep json | tr '\n' ' ')\"" | tee "$out/system.txt"
stop; vm "rm -rf $cfg"; start '~/perf'; q key Escape; sleep 1; stop
start '~/perf/big'; focus
vm "grep -m3 -i -E 'adapter|backend|llvmpipe|lavapipe|vulkan' ~/frames.log || true" | tee -a "$out/system.txt"

# 1. Scrolling the 100 000-file folder: 180 wheel notches at 60 Hz, then back up
q move 800 400; sleep 1
a=$(mark); q wheel 180 down 60; q wheel 180 up 60; sleep 1; b=$(mark)
q shot "$out/scroll.png"
f=$(fr "$a" "$b"); log "FRAMES scroll   $f"

# 2. Window resize: ten size changes between two sizes (RESIZE_CMD per desktop)
a=$(mark)
# shellcheck disable=SC2034 # RESIZE_CMD may read $i
for i in $(seq 10); do eval "${RESIZE_CMD:-q click 1303 25}"; sleep 1.2; done
sleep 1; b=$(mark); q shot "$out/resize.png"
f=$(fr "$a" "$b"); log "FRAMES resize   $f"

# 3. Switching views in the 100 000-file folder: Thumbnails, List, Columns, Tree, Details, twice
focus; a=$(mark)
for _ in 1 2; do for v in Thumbnails List Columns Tree Details; do view "View: $v"; q shot "$out/view-$v.png"; done; done
sleep 1; b=$(mark)
f=$(fr "$a" "$b"); log "FRAMES views    $f"

"$here/dvm.sh" "$d" get /home/atlas/frames.log "$out/frames.log"; rm -f "$out/frames.part"
stop
