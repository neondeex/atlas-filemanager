# Atlas on Linux and Windows: measurements

These numbers come from November 2026. The harness is in [scripts/linux-desktop-test](../scripts/linux-desktop-test/README.md):
`perf.sh`, `frames.sh` and `gui-test.sh` on Linux, and `windows/win-bench.ps1` on Windows. Every Linux
desktop ran in a QEMU/KVM VM with a 1366×768 virtio-vga screen and no GPU, so Atlas rendered there in
software (Mesa llvmpipe/lavapipe). On Windows, Atlas ran on the GPU in a 1366×768 window.

## Machines and builds

| column | hardware | VM | rendering | desktop | date |
|---|---|---|---|---|---|
| **Windows** | Intel i7-3770 (4C/8T, 2012), 7.9 GB, SATA SSD; Windows 10 22H2 | – (bare metal) | **GPU**: Intel HD 4000, Direct3D 11 | Windows shell, 1920×1200 at 100 % | 2026-11-17 |
| **Arch** | Intel Core i5-1334U laptop (10 cores / 12 threads, 15 GB), NVMe | 4 vCPU / 4 GB | software (lavapipe) | sway | 2026-11-17/18 |
| **Debian 13** | Intel i5-4300U (2C/4T, 2013), 3.7 GB | **2 vCPU / 2 GB** | software (llvmpipe, Mesa 25.0) | sway 1.10 | 2026-11-17/18 |
| **Fedora 44** | same host as Arch | 4 vCPU / 4 GB | software (llvmpipe, LLVM 22) | GNOME 50 (mutter), btrfs | 2026-11-17/18 |
| **NixOS 26.11** | same host as Arch | 4 vCPU / 4 GB | software (lavapipe, LLVM 21) | GNOME (mutter), ext4 | 2026-11-18 |

The Arch, Fedora and NixOS VMs shared their host with other VMs and builds. Timed runs held a lock, so
no build or other measurement ran at the same time. Debian on the 10-core host was not measured.

Builds:

- **beta.3**: the released v0.1.6-beta.3 packages (Windows `atlas.exe`, Arch `.pkg.tar.zst`, Debian
  `.deb`, Fedora `.rpm`). Built with the `release` profile: thin LTO, `codegen-units = 1`, opt-level 3.
- **perf (df0dd5a)**: commit df0dd5a = upstream main 38817c6 plus the open Linux PRs, including the
  Wayland clipboard fix. Built with `--profile release-perf --features perf-log` and
  `CARGO_PROFILE_RELEASE_PERF_LTO=off`. `release-perf` uses `codegen-units = 16`, and LTO was switched off
  completely, because thin-local LTO ran the build host out of memory. The perf-log feature only adds
  timing log lines. Without LTO and with more codegen units the code is usually a little slower than
  the release build. No paired release/perf run was measured, so how far these frame times are from
  the release is unknown.
- **perf (38817c6)**: NixOS only. The flake's `atlas-perf` package built from upstream main 38817c6
  with `perf-log`, LTO off and `codegen-units = 16` (the same settings as above, without the Linux
  PRs).

## Results

Ranges are over all valid runs in the output folders. Cold start uses three launches per run. Arch
uses three perf.sh runs, Debian two, and Fedora and NixOS one each. Windows idle CPU and memory cover
five runs. A dash means not measured.

| measure | Windows (GPU) beta.3 | Arch beta.3 | Debian (i5-4300U, 2 vCPU) beta.3 | Fedora beta.3 | NixOS perf (38817c6) |
|---|---|---|---|---|---|
| cold start, launch → first frame | 1.32–1.95 s (median 1.41) ¹ | 0.94–1.32 s | 2.06–2.30 s | 0.80–1.04 s | 0.71–0.80 s |
| idle CPU, 30 s, % of one core | 0.10–0.36 % | 0.27–0.57 % | 0.17–0.23 % | 0.50 % | 0.27 % |
| idle memory | working set 108–124 MiB (private 50–52 MiB) ² | RSS 286–289 MiB | RSS 280–283 MiB | RSS 231 MiB | RSS 242 MiB |
| 100k-file folder, cold | 1.04 s count / 1.44 s settled ³ | 1.15–1.22 s | 3.20–3.25 s | 2.26 s ⁴ | 1.32 s |
| 100k-file folder, warm | 0.65 s / 1.64 s ³ | 1.00–1.01 s | 2.07–2.10 s | 1.01 s | 1.66 s |
| scroll 100k list, frames/s (wheel at 60 Hz) | ~30 fps (perf df0dd5a, not beta.3) ⁵ | 34.6–36.0 fps (beta.3) | 7.5–7.6 fps (beta.3) | 29.1 fps (beta.3) | 32.0 fps (perf 38817c6) |
| subfolder search, one name in 100 040 | not measured ⁶ | 0.10–0.20 s | 2.1–3.8 s | 0.20 s | 0.20 s |

Frame times from the perf builds (`frames.sh`, `frames.py`). **draw** is the CPU time to build one
frame (GPUI `[gpui-frame] total`). **gap** is the time between two of Atlas's render passes (`frame_summary` at `App::render`), the rate at which Atlas produces frames; it is not measured on screen. Values are
p50 / p95 / max in ms, with the count over 16.7 ms in brackets.

| scenario | Windows (GPU) perf df0dd5a | Arch perf df0dd5a | Debian (i5-4300U) perf df0dd5a | Fedora perf df0dd5a | NixOS perf 38817c6 |
|---|---|---|---|---|---|
| scroll, draw | 10.6 / 12.7 / 36.3 (3 of 188) | 2.9 / 5.0 / 5.6 (0 of 211) | 6.9 / 11.0 / 16.1 (0 of 50) | 3.9 / 5.3 / 7.0 (0 of 159) | 2.8 / 4.6 / 8.4 (0 of 190) |
| scroll, gap | 32.8 / 38.7 / 108.8 | 28.9 / 34.1 / 49.2 | 128.0 / 146.5 / 152.0 | 39.7 / 46.5 / 53.3 | 32.5 / 37.3 / 40.1 |
| resize ×10, draw | 9.3 / 14.6 / 15.8 (0 of 48) | 2.5 / 4.8 / 7.7 (0 of 55) | 7.1 / 10.9 / 11.3 (0 of 29) | 2.7 / 6.9 / 9.6 (0 of 61) | 2.6 / 6.2 / 6.6 (0 of 61) |
| resize ×10, gap | 24.6 / 80.4 / 104.6 | 26.2 / 44.8 / 63.3 | 130.5 / 180.3 / 180.3 | 37.8 / 64.8 / 71.7 | 34.6 / 59.5 / 93.0 |
| view switch ×10, draw | 12.7 / **86.9** / 223.5 (72 of 249) | 3.5 / 11.5 / 64.3 (13 of 360) | 6.7 / 24.1 / 53.5 (44 of 325) | 4.3 / 17.6 / 111.3 (20 of 342) | 3.9 / 13.3 / 138.0 (13 of 348) |
| view switch ×10, gap | 36.9 / 123.8 / 244.8 | 27.0 / 78.2 / 245.2 | 9.7 / 217.0 / 243.0 | 32.9 / 85.3 / 237.9 | 34.3 / 85.3 / 238.8 |

On sway a lone window is tiled, so the Arch resize step is `swaymsg floating toggle` (ten toggles); on GNOME it is Super+Up/Down.

GUI checks (`gui-test.sh`, 18 checks):

| | beta.3 | perf df0dd5a (with the Wayland clipboard fix) |
|---|---|---|
| Arch, sway | 12/18. 4 clipboard checks fail: Atlas offers nothing to `wl-paste`, and pastes nothing from `wl-copy` (upstream issue #22). Idle CPU 1.00 % and RSS 350 MiB land exactly on the 1 % / 350 MiB limits. An early harness version ran this. | **18/18** (idle 0.17 %, RSS 343 MiB) |
| Debian 13, sway | 16/18: file copy/paste with Nautilus fails both ways (#22). Text copy works. | 17/18. The 100k-folder rename fails: F2 on the last of 100 000 entries while the folder is still loading leaves no rename (cause not confirmed). |
| Fedora 44, GNOME | 16/18: file copy/paste with Nautilus fails both ways. mutter has no data-control protocol, so the #22 fix does not help on GNOME. | 16/18, same two |
| NixOS, GNOME | – | 16/18 (perf 38817c6), the same two GNOME clipboard checks |
| Windows | Different checklist, checked by hand (Explorer interop, context menu, drag and drop, Recycle Bin, disk cleanup, private folder): all pass except search, which was not verified because the search service was not installed. Analyze Space on C: asks for admin rights (UAC) at once. | – |

Notes:

1. On Windows, cold start runs from launch to the first `[gpui-frame]` log line. On Linux, it runs to the
   first `wl_surface.attach` in Atlas's `WAYLAND_DEBUG` log. Both mark the first frame. Windows purged
   the standby list first; Linux dropped the page cache.
2. A Windows working set is not the same as a Linux RSS. In the Linux VMs, lavapipe keeps its render
   buffers in process memory; on Windows, Direct3D keeps them in video memory.
3. Windows timed folder opens from the screen: the first number is when the status-bar count changes,
   the second is when the list stops changing for 1.5 s. Linux times them from the end of the burst of
   Wayland commits.
4. Fedora uses btrfs, and its cold metadata reads are slow: a cold `ls -l` of the same folder takes
   3.65 s in that VM. Atlas's 2.26 s is faster than that.
5. The release binary logged too few frame lines while wheel input was injected. The perf binary
   showed about 30 fps (gap p50 32.8 ms), so that is the number used.
6. On Windows beta.3, Ctrl+Shift+F only toggles Expanded Search, and without the installed service the
   index is empty. The run logged 0.11 s, but that number is invalid.

### Known issue: the welcome screen while idle

Measured with `welcome-diag.sh` on 2026-11-18, in the NixOS GNOME VM (4 vCPU / 4 GB, software rendering), perf build 38817c6:

| state (20 s idle) | Atlas CPU, % of one core | frames/s |
|---|---|---|
| first-run welcome, profile field focused | 88.1 % (72.5 % without perf logging) | 1.8 |
| welcome, after clicking the background | 82.6 % (72.2 % without perf logging) | 1.8 |
| file browser | 1.9 % | 0.2 |
| file browser, search field focused | 2.2 % | 0.2 |

The welcome screen puts the focus on its profile field on every render. That field's blinking caret
redraws the whole full-screen overlay twice a second. GPUI builds each frame in 4–5 ms, but the
software renderer needs about 0.5 CPU-seconds to rasterize it. On a GPU this costs little; without one
it keeps most of a core busy until the welcome screen is closed. Not fixed.

## What can be compared

- **The 16.7 ms budget (60 fps) for draw time.** Draw time is CPU work in Atlas: layout, prepaint and
  paint. It hardly depends on the renderer. Scroll and resize stay inside the budget at p95 everywhere,
  even on the 2013 dual-core (on Windows, 3 of 188 scroll frames went over it). View switching in the
  100 000-file folder is the exception: p95 is 11.5–24 ms on Linux and 87 ms on Windows, with single frames of 54–138 ms on Linux and 224 ms on Windows. That is the first layout of
  a new view over 100k items, and it is the one place where Atlas's own frame cost shows.
- **The same order of magnitude on the same host.** Arch, Fedora and NixOS share the CPU, the VM
  size and the kind of renderer. In each, cold start takes about 1 s, idle CPU stays under 0.6 %,
  memory sits at 230–290 MiB, and search takes 0.1–0.2 s. Distro, compositor (sway vs mutter), filesystem
  and build still differ between them, so a small difference cannot be attributed to any one of these.
- **Idle CPU** is comparable everywhere: well under 1 % of a core on every system once the file
  browser is shown.
- **Build time vs. frame interval.** While scrolling and resizing, on both Windows and Linux, the draw
  p50 (2.5–10.6 ms) is far below the gap p50 (25–40 ms; 128–131 ms on the i5-4300U). Most of each interval is therefore spent outside building
  the frame. The gap runs between two of Atlas's render callbacks (`frame_summary`). These logs do not
  show whether the rest goes to presentation, the compositor's frame callback, or waiting on input.

## What cannot be compared

- **Different CPUs.** The Windows i7-3770, the i5-4300U in a 2-vCPU VM, and 4 vCPUs of the
  10-core host are different machines. Debian on the i5-4300U is about 2.5× slower per core and has half the
  vCPUs, which is why its cold start and folder times are about twice Arch's.
- **Software rendering caps the frame rate.** llvmpipe/lavapipe rasterize 1366×768 on the CPU, and the
  compositor (sway, mutter) also renders in software. That caps the Linux VMs at about 30–36 fps on 4
  vCPUs and about 8 fps on 2 slow vCPUs. A Linux gap or frames/s figure measures the VM, not Atlas on a
  real GPU. The Windows ~30 fps on a GPU is a separate finding: draw takes 10.6 ms, but Atlas still
  renders only every ~33 ms while scrolling.
- **btrfs cold cache.** On Fedora, the cold 100k open measures btrfs metadata reads (note 4).
- **GNOME vs sway.** mutter commits twice per frame and sway once; `wdlog.py` counts buffer attaches
  for this reason. Under mutter, Fedora and NixOS scroll at about 29–32 fps, against 35 on sway.
- **beta.3 vs perf builds.** The perf builds have no LTO and use 16 codegen units. No release run of
  frames.sh exists to compare them with, so the frame table is about the perf builds only.

## Sources

The raw results are the `results.txt` files and the screenshots in each output folder
(`perf-<distro>`, `frames-<distro>`, `gui-<distro>`, plus the Windows `perf-windows`,
`perf-windows-main` and `frames-windows`), produced by the scripts above. The Windows GUI checks
(`gui-windows`) were done by hand in the console session, with `windows/win-do.sh`.
