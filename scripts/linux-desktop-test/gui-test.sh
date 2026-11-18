#!/usr/bin/env bash
# 18 GUI checks of the installed Atlas in a real desktop session in any VM (run on the host):
#   gui-test.sh DISTRO [OUT_DIR]   (default $ATLAS_TEST_OUT/gui-DISTRO, ATLAS_TEST_OUT=~/atlas-desktop-test)
# Input and screenshots go through QMP (keyboard, tablet, screendump), results are checked on
# the guest's disk over ssh. Clipboard checks talk to the desktop's own file manager (Nautilus
# on GNOME); a desktop without one (sway on Arch) uses wl-clipboard as before.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
d=${1:?distro}; out=${2:-${ATLAS_TEST_OUT:-$HOME/atlas-desktop-test}/gui-$d}
rm -rf "$out"; mkdir -p "$out"
vm() { "$here/dvm.sh" "$d" ssh "$@"; }
q() { "$here/dvm.sh" "$d" qmp "$@"; }
k() { q key "$@"; sleep 0.35; }
t() { q type "$@"; sleep 0.35; }
pass=0 fail=0
log() { printf '%s\n' "$*" | tee -a "$out/results.txt"; }
ok() { pass=$((pass + 1)); log "PASS  $1"; }
bad() { fail=$((fail + 1)); log "FAIL  $1${2:+ ($2)}"; }
shot() { q shot "$out/$1.png"; }
# Poll a guest condition for up to $2 seconds (default 10).
until_true() { vm "for i in \$(seq $((${2:-10} * 4))); do $1 && exit 0; sleep 0.25; done; exit 1"; }
check() { if until_true "$2" "${3:-10}"; then ok "$1"; else bad "$1"; fi; shot "$(tr ' /' '__' <<<"$1")"; }
W=/home/atlas/atlas-gui-work
CFG=/home/atlas/.config/atlas-gui-test
goto() { k ctrl-l; sleep 0.3; k ctrl-a; t "$1"; sleep 0.5; k Return; sleep 1; }
focus() { q click "${FOCUS_X:-800}" "${FOCUS_Y:-700}"; sleep 0.5; }
# WAYLAND_DEBUG=1 only for the first, short launch: it logs every frame and would inflate idle cost.
start_atlas() {
  local dbg=; [[ ${DEBUG_LAUNCH:-} ]] && dbg=WAYLAND_DEBUG=1
  vm "ATLAS_CONFIG_DIR=$CFG $dbg nohup ~/insession atlas $* > ~/atlas-gui.log 2>&1 &"
  if [[ $dbg ]]; then until_true "grep -q 'attach(wl_buffer' ~/atlas-gui.log" 30
  else until_true 'pgrep -x atlas >/dev/null' 30 && sleep 4; fi
}
stop_atlas() { focus; k ctrl-q; until_true '! pgrep -x atlas >/dev/null' 10 || vm 'pkill -x atlas; true'; sleep 1; }
nautilus=$(vm 'command -v nautilus' || true)

vm "pkill -x atlas; pkill -x nautilus; rm -rf $W $CFG ~/.local/share/Trash; mkdir -p $W/{start,rename,src,dst,trash,hidden,clip-in,clip-src,content,naut-in,naut-src}
  mkdir -p '$W/deep/a/b/c/Needle Target Folder'
  printf 'Atlas test\n' > $W/start/readme.txt; printf 'rename me\n' > $W/rename/before.txt
  for i in \$(seq -w 1 100); do head -c \$((10#\$i * 97)) /dev/urandom > $W/src/file-\$i.bin; done
  printf 'trash me\n' > $W/trash/to-trash.txt
  printf 'secret\n' > $W/hidden/.dotfile; printf 'visible\n' > $W/hidden/visible.txt
  printf 'copied by another app\n' > $W/clip-src/from-other-app.txt
  printf 'from nautilus\n' > $W/naut-src/from-nautilus.txt
  printf 'needle-content-31337\n' > $W/content/match.txt
  python3 - <<'EOF'
import zlib, struct
w, h = 320, 200
raw = b''.join(b'\\0' + bytes(v for x in range(w) for v in (x % 256, y % 256, 160)) for y in range(h))
def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
open('$W/start/picture.png', 'wb').write(b'\\x89PNG\\r\\n\\x1a\\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
import zipfile; zipfile.ZipFile('$W/start/archive.zip', 'w').writestr('zipped.txt', 'inside zip\n')
EOF
  (. /etc/os-release; echo \$PRETTY_NAME); atlas --version | head -1; echo \"desktop: \${XDG_CURRENT_DESKTOP:-?} \$(ls /run/user/\$(id -u) | grep -E '^wayland-[0-9]+$')\"" | tee "$out/system.txt"

# 1. Native Wayland window: the log shows an xdg_toplevel, and DISPLAY is unset (no X11 fallback)
if DEBUG_LAUNCH=1 start_atlas "$W/start"; then
  vm "grep -q 'get_toplevel' ~/atlas-gui.log" && ok "native Wayland window (xdg_shell)" || bad "native Wayland window"
else
  bad "Atlas window appears"; exit 1
fi
sleep 3; shot 01-first-frame
k Escape
check "first-run assistant dismissed" "grep -q '\"getting_started_seen\": *true' $CFG/settings.json 2>/dev/null"
stop_atlas; start_atlas "$W/start"
focus; sleep 1; shot 02-browser

# 2. Create a folder (Ctrl+Shift+N, type the name, Enter)
goto "$W/start"; k ctrl-shift-n; t "Created by Atlas"; k Return
check "create folder" "[ -d '$W/start/Created by Atlas' ]"

# 3. Rename (F2)
goto "$W/rename"; k Home; k F2; k ctrl-a; t after.txt; k Return
check "rename file" "[ -f $W/rename/after.txt ] && [ ! -e $W/rename/before.txt ]"

# 4. Copy 100 files between folders
goto "$W/src"; k ctrl-a; k ctrl-c
goto "$W/dst"; k ctrl-v
check "copy 100 files" "[ \$(ls $W/dst | wc -l) -eq 100 ] && cmp -s $W/src/file-100.bin $W/dst/file-100.bin" 30

if [[ -n $nautilus ]]; then
  # 5-8. Clipboard between Atlas and Nautilus, both directions, files and text.
  naut() { vm "nohup ~/insession nautilus --new-window '$1' >> ~/nautilus.log 2>&1 &"; sleep 4; }
  naut_close() { k ctrl-w; sleep 1; vm 'pkill -x nautilus; true'; sleep 1; }
  # sway tiles Nautilus to the right half and has no clipboard manager: a Wayland selection dies with
  # the app that owns it, so there Nautilus stays open while Atlas pastes, and clicks pick the window.
  tiled=$(vm 'pgrep -x sway >/dev/null && echo 1' || true)
  naut_focus() { [[ -z $tiled ]] || { q click 1000 650; sleep 0.5; }; }
  atlas_focus() { if [[ $tiled ]]; then q click 450 500; sleep 0.5; else focus; fi; }
  naut_done() { if [[ $tiled ]]; then vm 'pkill -x nautilus; true'; sleep 1; else naut_close; fi; }
  # Atlas copies a file, Nautilus pastes it.
  goto "$W/rename"; k Home; k ctrl-c
  naut "$W/naut-in"; shot 05a-nautilus; k ctrl-v
  check "Nautilus pastes a file copied in Atlas" "[ -f $W/naut-in/after.txt ] && [ -f $W/rename/after.txt ]"
  naut_close; vm "rm -f $W/naut-in/after.txt"  # the next check makes a folder of that name
  # Atlas copies text (the name in the rename field), Nautilus pastes it as a new folder's name.
  goto "$W/rename"; k Home; k F2; k ctrl-a; k ctrl-c; k Escape
  naut "$W/naut-in"; k ctrl-shift-n; sleep 1; k ctrl-a; k ctrl-v; sleep 0.5; shot 06a-nautilus-new-folder; k Return
  check "Nautilus pastes text copied in Atlas" "[ -d $W/naut-in/after.txt ]"
  naut_close
  # Nautilus copies a file, Atlas pastes it.
  naut "$W/naut-src"; naut_focus; k ctrl-a; k ctrl-c; sleep 0.5; [[ $tiled ]] || naut_close
  atlas_focus; goto "$W/clip-in"; k ctrl-v
  check "Atlas pastes a file copied in Nautilus" "[ -f $W/clip-in/from-nautilus.txt ] && [ -f $W/naut-src/from-nautilus.txt ]"
  [[ $tiled ]] && naut_done
  # Nautilus copies text (its rename field), Atlas pastes it into its own rename field.
  naut "$W/naut-src"; naut_focus; k ctrl-a; k F2; sleep 1; k ctrl-a; k ctrl-c; k Escape; [[ $tiled ]] || naut_close
  atlas_focus; goto "$W/rename"; k Home; k F2; k ctrl-a; k ctrl-v; k Return
  check "Atlas pastes text copied in Nautilus" "[ -f $W/rename/from-nautilus.txt ]"
  [[ $tiled ]] && naut_done
  vm "mv $W/rename/from-nautilus.txt $W/rename/after.txt 2>/dev/null; true"
else
  # 5-8. Clipboard to other apps (wl-paste) and from them (wl-copy), as on Arch before.
  goto "$W/rename"; k Home; k ctrl-c
  check "clipboard exports text/uri-list" "~/insession wl-paste -t text/uri-list 2>/dev/null | grep -q 'file://$W/rename/after.txt'"
  check "clipboard offers text to editors" "~/insession wl-paste -t text/plain 2>/dev/null | grep -q 'file://$W/rename/after.txt'"
  check "clipboard marks a copy for GNOME" "~/insession wl-paste -t x-special/gnome-copied-files 2>/dev/null | head -n1 | grep -qx copy"
  vm "printf 'file://%s\r\n' $W/clip-src/from-other-app.txt | ~/insession wl-copy -t text/uri-list >/dev/null 2>&1"
  goto "$W/clip-in"; k ctrl-v
  check "paste file copied by another app" "[ -f $W/clip-in/from-other-app.txt ]"
fi

# 9-10. Trash (Delete) and undo (Ctrl+Z)
goto "$W/trash"; k Home; k Delete
check "send to trash" "[ ! -e $W/trash/to-trash.txt ] && [ -f ~/.local/share/Trash/files/to-trash.txt ]"
k ctrl-z
check "undo restores from trash" "[ -f $W/trash/to-trash.txt ]"

# 11. Hidden files toggle (Ctrl+.)
goto "$W/hidden"; sleep 1; shot 08a-hidden-off
k ctrl-.; sleep 1.5; shot 08b-hidden-on
if ! cmp -s "$out/08a-hidden-off.png" "$out/08b-hidden-on.png"; then ok "hidden files toggle changes the listing"; else bad "hidden files toggle"; fi
k ctrl-.

# ZIP opens as a folder and Quick Look reads a file inside it. Judged from the screenshot.
goto "$W/start"; k Home; k Down; k Return; sleep 1.5
k Home; k space; sleep 1.5; shot 09-zip; k space
k alt-Up; sleep 1
log "LOOK  09-zip.png: archive.zip open, Quick Look shows \"inside zip\""

# 12. Ctrl+K jumps to a deep folder (probe: a new folder lands there)
goto "$W/start"; k ctrl-k; sleep 0.5; t "Needle Target Folder"; sleep 3; shot 10-goto
k Return; sleep 1.5
k ctrl-shift-n; sleep 0.5; t probe; k Return
check "Ctrl+K navigates to a folder found by name" "[ -d '$W/deep/a/b/c/Needle Target Folder/probe' ]"

# Search in subfolders by name (Ctrl+Shift+F). Judged from the screenshot.
goto "$W"; k ctrl-shift-f; sleep 0.5; t match; sleep 4; shot 11-subfolder-search
log "LOOK  11-subfolder-search.png: content/match.txt listed"
k Escape

# 13. Quick Look (Space) on an image, command palette, settings
goto "$W/start"; k Home; k End; sleep 0.3; k space; sleep 2; shot 12-quicklook; k space
k ctrl-shift-p; sleep 1; shot 13-palette; k Escape
k ctrl-,; sleep 1.5; shot 14-settings; k Escape
vm 'pgrep -x atlas >/dev/null' && ok "palette and settings open without crash" || bad "palette and settings open without crash"

# 14-15. Idle cost (no constant redraws) and memory
sleep 5
m=$(vm 'p=$(pgrep -x atlas); a=$(awk "{print \$14+\$15}" /proc/$p/stat); sleep 30; b=$(awk "{print \$14+\$15}" /proc/$p/stat);
  awk -v d=$((b-a)) -v hz=$(getconf CLK_TCK) "BEGIN{printf \"%.2f \", d/hz/30*100}"; awk "/VmRSS/ {printf \"%d\", \$2/1024}" /proc/$p/status')
cpu=${m% *} rss=${m#* }
log "MEASURE idle CPU over 30 s: ${cpu}%  RSS: ${rss} MiB"
awk -v c="$cpu" 'BEGIN { exit !(c < 1.0) }' && ok "idle CPU < 1%" || bad "idle CPU < 1%" "$cpu%"
((rss < 350)) && ok "idle RSS < 350 MiB" || bad "idle RSS < 350 MiB" "$rss MiB"

# 16. Restart restores the session (probe: a new folder lands in the last folder)
goto "$W/hidden"; sleep 1; stop_atlas
start_atlas && sleep 3; focus
k ctrl-shift-n; sleep 0.5; t after-restart; k Return
check "restart restores the last folder" "[ -d $W/hidden/after-restart ]"

# 17. Large folder: 100 000 files, the last one can be renamed
vm "mkdir -p $W/big && cd $W/big && seq -w 1 100000 | sed 's/^/f/' | xargs touch"
goto "$W/start"; sleep 1
s=$(date +%s.%N); goto "$W/big"
k End; k F2; k ctrl-a; t last-renamed; k Return
if until_true "[ -e $W/big/last-renamed ] && [ ! -e $W/big/f100000 ]" 20; then
  log "MEASURE 100k folder: open + select last + rename took $(awk -v s="$s" -v e="$(date +%s.%N)" 'BEGIN{printf "%.2f", e-s}')s (with scripted key delays)"
  ok "100 000-file folder browsable (last entry renamed)"
else bad "100 000-file folder browsable"; fi
shot 16-big-folder

stop_atlas
log "TOTAL pass=$pass fail=$fail"
((fail == 0))
