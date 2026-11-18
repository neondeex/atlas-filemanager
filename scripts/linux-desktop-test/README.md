# Linux desktop test VMs

These scripts test the installed Atlas in a real Wayland desktop inside a QEMU/KVM VM. They use the
keyboard, the mouse and the screen and need no one at the machine. The scripts run on the host, except `insession`, `search-probe.py`,
`provision-sway.sh` and `provision-gnome.sh`, which run in the guest; input and screenshots go through
QMP, and the results are checked over ssh on the guest's disk.
The numbers are in [docs/linux-performance.md](../../docs/linux-performance.md).

## Requirements

- Linux host with `/dev/kvm`, `qemu-system-x86_64`, `qemu-img`, `cloud-localds` (cloud-image-utils),
  `ssh`/`scp`, `curl` and `python3`. NixOS images also need Docker.
- About 4 GB of RAM and 10–40 GB of disk per VM (defaults: 4 vCPU, 4096 MiB).
- The display is a 1366x768 virtio-vga screen with no GPU, so Atlas renders in software
  (llvmpipe/lavapipe).

| variable | default | used by |
|---|---|---|
| `ATLAS_VM_ROOT` | `~/vm` | VM directories `atlas-<distro>` (disk, ssh key, QMP socket, serial log) |
| `DVM_CPUS`, `DVM_MEM` | `4`, `4096` | `dvm.sh up` |
| `ATLAS_TEST_OUT` | `~/atlas-desktop-test` | results of gui-test, perf and frames (`<kind>-<distro>/`) |
| `ATLAS_BIN` | `~/atlas-perf/atlas` (guest) | `frames.sh`: the perf-log binary |
| `RESIZE_CMD` | click on the maximize button | `frames.sh`: how one resize step is done on this desktop |
| `FOCUS_X`, `FOCUS_Y` | 800, 560/700 | a point inside Atlas's file list |
| `ATLAS_WIN_HOST` | – (required) | `windows/`: ssh host of the Windows PC |

## VMs

`dvm.sh DISTRO up|ssh [cmd]|put SRC DST|get SRC DST|qmp ARGS|down|destroy`. The ssh ports are
127.0.0.1:2222 (arch), 2223 (debian), 2224 (fedora) and 2225 (nixos). The first `up` downloads the cloud
image, checks its checksum, makes an ssh key and a cloud-init seed with user `atlas`, and boots.
After that, install a desktop and the Atlas package as root:

```sh
./dvm.sh arch up                                   # same for debian, fedora
./dvm.sh arch put atlas-*.pkg.tar.zst /tmp/        # .deb for Debian, .rpm for Fedora
./dvm.sh arch put provision-sway.sh /tmp/
./dvm.sh arch ssh 'sudo bash /tmp/provision-sway.sh /tmp/atlas-*.pkg.tar.zst && sudo reboot'
```

- **Arch, Debian 13: sway** (`provision-sway.sh`). Autologin on tty1 starts sway, with no XWayland and
  no clipboard manager. Nautilus is installed for the clipboard checks.
- **Fedora 44, Debian 13: GNOME** (`provision-gnome.sh`). GDM autologin, Wayland, no lock screen.
- **NixOS: GNOME** (`nixos-image.sh FLAKE_LOCK`). It builds `atlas-nixos/base.qcow2` with the same
  nixpkgs as Atlas's `flake.lock`, inside the `nixos/nix` container. Then use `nix-docker.sh` /
  `nix-build-atlas.sh` to build Atlas from the flake and copy it in, or use `nix profile install` in
  the guest.

On every GNOME VM (Fedora, Debian, NixOS), GNOME opens the Activities overview after login. Wait until
`gnome-shell` runs, then press Escape once (`./dvm.sh fedora qmp key Escape`) before you start any test.
Otherwise the first Escape of a test closes the overview instead of Atlas's welcome screen, and every
measurement runs on the welcome screen.

`insession` goes to the guest's home (`./dvm.sh X put insession /home/atlas/`). It runs a command
inside the desktop session from ssh, and every test uses it. `qmp.py` drives the VM: `key`, `type`,
`click`, `drag`, `wheel`, `shot FILE.png`, `region` (save a screen region) and `settle` (seconds until a
screen region stops changing; given a saved region, only once it also differs from that region).

## gui-test.sh DISTRO: 18 checks

1. Native Wayland window (xdg_toplevel, no X11 fallback).
2. The first-run assistant closes with Escape.
3. Create a folder.
4. Rename with F2.
5. Copy 100 files between folders.
6. Clipboard with the desktop's file manager: Nautilus pastes a file copied in Atlas.
7. Nautilus pastes text copied in Atlas.
8. Atlas pastes a file copied in Nautilus.
9. Atlas pastes text copied in Nautilus.

   Without Nautilus, checks 6–9 test against `wl-copy`/`wl-paste` instead: text/uri-list, text/plain
   and the GNOME copy marker are offered, and a file copied by another app is pasted.
10. Send to trash.
11. Undo restores from trash.
12. Hidden files toggle.
13. Ctrl+K finds a deep folder by name.
14. Palette and settings open without a crash.
15. Idle CPU below 1 % over 30 s.
16. Idle RSS below 350 MiB.
17. A restart restores the last folder.
18. In a 100 000-file folder, the last entry can be renamed.

Two checks are judged by eye from their screenshots and are not counted: ZIP opens as a folder with
Quick Look inside it, and subfolder search. Results go to `results.txt` (`PASS`/`FAIL`/`MEASURE`/`LOOK`
lines, `TOTAL`) with one screenshot per check. The exit code is 0 only when all 18 pass.

## perf.sh DISTRO: startup and responsiveness of the installed build

Timings come from Atlas's own `WAYLAND_DEBUG=1` log in the guest (`wdlog.py`), so they mean the same on
every desktop:

- cold start ×3 after `drop_caches`: launch → first buffer attach, and → the end of the first burst of
  commits;
- idle CPU (% of one core over 30 s) and RSS/PSS;
- opening a 100 000-file folder, cold and warm: Enter → the end of the commit burst;
- scrolling it: 180 wheel notches at 60 Hz, frames (buffer attaches) per second and frame gaps;
- subfolder search for one name among 100 040 entries, timed from screenshots (`qmp.py settle`).

## frames.sh DISTRO: frame times of a perf-log build

You need a perf-log build (for example `cargo build --profile release-perf -p atlas
--features perf-log`), copied to `~/atlas-perf/` in the guest. It runs with the switches of
`scripts/run-release-perf.ps1` (`ATLAS_PERF=1 ATLAS_PERF_EVENTS=1 ATLAS_PERF_MS=4 GPUI_FRAME_LOG=0`)
and measures three cases: scrolling the 100k folder (180 notches down and up), 10 window resizes, and
switching views twice (Thumbnails, List, Columns, Tree, Details). `frames.py` prints p50/p95/max and
the count over 16.7 ms for:

- **draw**: GPUI's `[gpui-frame] total`, the CPU time to build one frame;
- **gap**: the time between two of Atlas's render passes (`frame_summary`), the rate at which Atlas produces frames while something moves.

On sway, use `RESIZE_CMD="vm '~/insession swaymsg floating toggle'"`. On GNOME,
use `RESIZE_CMD='if ((i % 2)); then q key meta_l-Up; else q key meta_l-Down; fi'`.

## Other tools

- `welcome-diag.sh DISTRO BIN OUT`: CPU and frames per second of a perf-log build, over 20 s, in four
  idle states (welcome screen with the field focused, welcome after clicking the background, file
  browser, browser with the search field focused).
- `search-probe.py TEXT`, run in the guest: asks the running `atlas-search-service` directly and prints
  the paths it finds.

## Windows (`windows/`)

The same measurements on a real Windows PC, driven over OpenSSH from the Linux host
(`ATLAS_WIN_HOST=<ssh host>`, an admin account):

1. Copy `atlas.exe` and `atlas-search-service.exe` to a folder on the PC.
2. Make sure a user is logged on at the console and has been idle for at least 15 minutes, and that no
   cargo build is running there. The scripts check both before they copy anything, and refuse otherwise
   (`windows/lib.sh` reads the console session's idle time from `quser`).
3. `windows/win-perf.sh perf 'C:\path\to\exe-dir' out/perf-windows` (keep the quotes) measures cold start ×3 (the standby
   list is purged first, the counterpart of `drop_caches`), idle CPU and working set, the 100k folder
   cold/warm, and scroll. Use `frames` with a perf-log `atlas.exe` for the frame times; analyse them
   with `frames.py frames.log FROM TO` using the line ranges in `marks.txt`.
4. `win-bench.ps1` runs in the console session through a scheduled task (`win-task.ps1`), never in
   SSH session 0. There is no scripted GUI checklist on Windows: the Windows GUI checks in the doc were
   done by hand, with `win-do.sh start|'snippet'|get|stop`, which runs single PowerShell snippets in
   that session.
5. Clean up: `rmdir /s /q C:\Users\Public\atlas-cmp-run`.
