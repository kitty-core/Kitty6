r"""Writes the run table of tools/charts/probe_table.py as the CSV the
Kitty6 Playability page reads (https://kitty-core.github.io, data/probe-runs.csv).

    python tools/charts/site_csv.py [folder] [-o out.csv]   # default folder: %LOCALAPPDATA%\victory\probes

Columns: order, run, saved, world_s, then probe_table's columns, then focused
(percent of the last 45 s with the window in front, blank if not recorded).
"""
import argparse
import csv
import datetime
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from probe_table import COLUMNS, summary  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("folder", nargs="?", default=os.path.join(os.environ.get("LOCALAPPDATA", ""), "victory", "probes"))
    ap.add_argument("-o", "--out", default="probe-runs.csv")
    a = ap.parse_args()
    rows = []
    for path in sorted(glob.glob(os.path.join(a.folder, "*.csv")), key=os.path.getmtime):
        name = os.path.basename(path)[:-4]
        if name.startswith("probe-"):
            continue
        s = summary(path)
        if s is None:
            continue
        saved = datetime.datetime.fromtimestamp(os.path.getmtime(path)).strftime("%Y-%m-%dT%H:%M")
        values = s[1]
        values[-1] = values[-1].rstrip("%")
        rows.append([len(rows) + 1, name, saved, s[0]] + values)
    with open(a.out, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["order", "run", "saved", "world_s"] + COLUMNS + ["focused"])
        w.writerows(rows)
    print(f"{len(rows)} runs -> {a.out}")


if __name__ == "__main__":
    main()
