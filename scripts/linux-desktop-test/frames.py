#!/usr/bin/env python3
"""Frame times from a perf-log Atlas run (GPUI_FRAME_LOG=0, ATLAS_PERF=1, ATLAS_PERF_EVENTS=1).

  frames.py LOG FROM_LINE TO_LINE
Prints, for the log lines in [FROM_LINE, TO_LINE):
  draw: GPUI [gpui-frame] total = CPU time to build one frame (layout, prepaint, paint), the
        number upstream's scripts/analyze-frame-phases.py reads;
  gap:  Atlas frame_summary gap_ms = time between two of Atlas's render passes (App::render),
        the rate at which Atlas produces frames while something moves; not measured on screen.
        Gaps over 250 ms are idle time and are left out.
Target from the store listing: 60+ FPS = 16.7 ms per frame.
"""
import math, re, sys

def pct(values, p):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * p) - 1)]

def describe(name, values):
    if not values:
        return f"{name}: n=0"
    over = sum(1 for v in values if v > 16.7)
    return (f"{name}: n={len(values)} p50={pct(values, .5):.2f}ms p95={pct(values, .95):.2f}ms "
            f"max={max(values):.2f}ms over16.7={over}")

def main():
    path, a, b = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    lines = open(path, errors="replace").read().splitlines()[a:b]
    draw = [float(m[1]) for l in lines if (m := re.search(r"\[gpui-frame\] total=([\d.]+)ms", l))]
    gap = [float(m[1]) for l in lines if "frame_summary" in l and (m := re.search(r"gap_ms=([\d.]+)", l))]
    gap = [g for g in gap if g <= 250]
    print(describe("draw", draw) + " | " + describe("gap", gap))

main()
