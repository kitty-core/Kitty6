r"""Prints a markdown table of saved probe runs (each run's medians over its
last 45 s in the world, i.e. seconds with 500 draws a frame or more).

    python tools/charts/probe_table.py [folder]   # default: %LOCALAPPDATA%\victory\probes

Only runs saved under a name are listed (probe-<n>.csv files are skipped).
"""
import csv
import datetime
import glob
import os
import statistics
import sys

COLUMNS = ["fps", "ms_p95", "prep_ms", "gpu_ms", "private_mb", "avail_ram_mb", "sys_pages_in_per_s", "vram_used_mb"]


def summary(path):
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f))
    world = [r for r in rows if float(r.get("draws") or 0) >= 500]
    if len(world) < 10:
        return None
    end = float(world[-1]["t_s"])
    last = [r for r in world if float(r["t_s"]) > end - 45]
    out = []
    for c in COLUMNS:
        values = [float(r[c]) for r in last if r.get(c) not in (None, "")]
        out.append(f"{statistics.median(values):.1f}" if values else "")
    focused = [r.get("focused") for r in last if r.get("focused") not in (None, "")]
    out.append(f"{sum(f == '1' for f in focused) / len(focused):.0%}" if focused else "")
    return len(world), out


def main():
    folder = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.environ["LOCALAPPDATA"], "victory", "probes")
    print("| run | saved | world s | " + " | ".join(COLUMNS) + " | focused |")
    print("|---" * (len(COLUMNS) + 4) + "|")
    for path in sorted(glob.glob(os.path.join(folder, "*.csv")), key=os.path.getmtime):
        name = os.path.basename(path)[:-4]
        if name.startswith("probe-"):
            continue
        s = summary(path)
        if s is None:
            continue
        saved = datetime.datetime.fromtimestamp(os.path.getmtime(path)).strftime("%m-%d %H:%M")
        print(f"| {name} | {saved} | {s[0]} | " + " | ".join(s[1]) + " |")


if __name__ == "__main__":
    main()
