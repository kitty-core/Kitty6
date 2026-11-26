# Draws Kitty6's README charts (docs/charts/*.svg, light and dark) from the
# frame probes victory writes (%LOCALAPPDATA%\victory\probes\<run>.csv, see
# docs/probes.md). Only our own measurements go in: no title data.
#
#   python tools/charts/charts.py docs/charts
import csv
import glob
import os
import statistics as st
import sys

import matplotlib

matplotlib.use("svg")
import matplotlib.pyplot as plt

plt.rcParams["svg.fonttype"] = "none"
plt.rcParams["font.family"] = ["Segoe UI", "Arial", "DejaVu Sans"]

PROBES = os.path.join(os.environ["LOCALAPPDATA"], "victory", "probes")
OUT = sys.argv[1] if len(sys.argv) > 1 else "docs/charts"
# The run shown second by second, and its world time.
RUN = "D2-pr237"
WORLD_SECONDS = 45

THEMES = {
    "light": dict(surface="#fcfcfb", ink="#0b0b0b", ink2="#52514e", muted="#898781", grid="#e1e0d9",
                  axis="#c3c2b7", s1="#2a78d6", s2="#eb6834", other="#c3c2b7"),
    "dark": dict(surface="#1a1a19", ink="#f0efec", ink2="#c3c2b7", muted="#898781", grid="#2c2c2a",
                 axis="#383835", s1="#3987e5", s2="#d95926", other="#52514e"),
}

# What changed at a run, for the progress chart.
NOTES = {
    "A-base": "start: upstream defaults",
    "F-mix4": "buffer copies\non the card",
    "H-merged1": "a merge brings back a lock\nthe recording threads queue on",
    "N-fix1": "lock fix: copies reused\noutside the global lock",
    "D2-pr237": "NVIDIA descriptor and\nreadback merges",
    "H-seriesx": "Series X\nprofile",
}
NOTES_BELOW = {"H-seriesx", "A-base", "H-merged1"}

# Each project's own figure as the GTA VI PC Race tracker listed it on 26 Nov 2026.
TRACKER = [
    ("Vice-Tay", 21.6), ("VIctory-7900XTX", 22.0), ("VIctory-NVIDIA", 21.0), ("Kitty6", 20.5),
    ("klbf-lab", 14.0), ("VI-BR-PORT", 12.0), ("VIctory-RDNA", 12.1), ("Kelvaroq", 10.0),
]


def preview(fig, stem):
    # CHARTS_PNG=<dir>: also PNGs, to look at.
    d = os.environ.get("CHARTS_PNG")
    if d:
        fig.savefig(os.path.join(d, stem + ".png"), facecolor=fig.get_facecolor(), dpi=110)


def rows(name):
    with open(os.path.join(PROBES, name + ".csv"), newline="") as f:
        return list(csv.DictReader(f))


def world(r):
    end = float(r[-1]["t_s"])
    return [x for x in r if float(x["t_s"]) >= end - WORLD_SECONDS and float(x["fps"]) > 0]


def runs():
    files = sorted(glob.glob(os.path.join(PROBES, "*.csv")), key=os.path.getmtime)
    out = []
    for f in files:
        name = os.path.basename(f)[:-4]
        if name.startswith("probe-"):
            continue
        w = world(rows(name))
        if len(w) >= 8:
            out.append((name, st.median(float(x["fps"]) for x in w)))
    return out


def style(ax, fig, t, ylabel=None):
    fig.patch.set_facecolor(t["surface"])
    ax.set_facecolor(t["surface"])
    for s in ("top", "right", "left"):
        ax.spines[s].set_visible(False)
    ax.spines["bottom"].set_color(t["axis"])
    ax.tick_params(colors=t["muted"], length=0, labelsize=9)
    ax.grid(axis="y", color=t["grid"], linewidth=0.8)
    ax.set_axisbelow(True)
    if ylabel:
        ax.set_ylabel(ylabel, color=t["ink2"], fontsize=9)


def title(fig, t, head, sub):
    fig.text(0.012, 0.965, head, color=t["ink"], fontsize=13, weight="bold", va="top")
    fig.text(0.012, 0.885, sub, color=t["ink2"], fontsize=9, va="top")


def ref(ax, t, y, label, left=False):
    ax.axhline(y, color=t["muted"], linewidth=1, linestyle=(0, (4, 3)), zorder=1)
    lo, hi = ax.get_xlim()
    ax.text(lo if left else hi, y + 0.4, label, color=t["muted"], fontsize=8, ha="left" if left else "right",
            va="bottom")


def progress(t, name, data):
    fig, ax = plt.subplots(figsize=(9, 4.2))
    fig.subplots_adjust(left=0.06, right=0.985, top=0.78, bottom=0.06)
    style(ax, fig, t, "fps (median, in the world)")
    xs = range(len(data))
    ys = [m for _, m in data]
    ax.plot(xs, ys, color=t["s1"], linewidth=2, zorder=2)
    ax.scatter(xs, ys, s=40, color=t["s1"], edgecolors=t["surface"], linewidths=2, zorder=3)
    ax.set_xticks([])
    ax.set_xlim(-0.6, len(data) - 0.4)
    ax.set_ylim(0, 33)
    ref(ax, t, 30, "title cap 30")
    ref(ax, t, 20, "tracker playable bar 20", left=True)
    for i, (n, m) in enumerate(data):
        if n in NOTES:
            below = n in NOTES_BELOW
            ax.annotate(NOTES[n], (i, m), xytext=(0, -12 if below else 9), textcoords="offset points",
                        ha="center", va="top" if below else "bottom", fontsize=8, color=t["ink2"])
    title(fig, t, "Kitty6 frame rate, run by run (26 Nov 2026)",
          f"{len(data)} scripted runs in order: opening scene, Series S profile, RTX 4070 + 32 GB. "
          "The dips are experiments that were reverted.")
    fig.savefig(f"{OUT}/progress-{name}.svg", facecolor=t["surface"])
    preview(fig, f"progress-{name}")
    plt.close(fig)


def one_run(t, name, r):
    tt = [float(x["t_s"]) for x in r]
    fps = [float(x["fps"]) for x in r]
    fig, ax = plt.subplots(figsize=(9, 3.6))
    fig.subplots_adjust(left=0.06, right=0.985, top=0.74, bottom=0.15)
    style(ax, fig, t, "fps")
    ax.plot(tt, fps, color=t["s1"], linewidth=2)
    ax.set_xlim(0, tt[-1])
    ax.set_ylim(0, 40)
    ax.set_xlabel("seconds since launch", color=t["muted"], fontsize=9)
    ref(ax, t, 30, "title cap 30")
    title(fig, t, "One run, second by second",
          "Launch and menus at the 30 cap, loading from about 55 s, then the opening scene from about 145 s (run D2).")
    fig.savefig(f"{OUT}/run-{name}.svg", facecolor=t["surface"])
    preview(fig, f"run-{name}")
    plt.close(fig)


def memory(t, name, r):
    tt = [float(x["t_s"]) for x in r]
    ram = [float(x["ws_mb"]) / 1024 for x in r]
    card = [float(x["vram_used_mb"]) / 1024 for x in r]
    fig, ax = plt.subplots(figsize=(9, 3.6))
    fig.subplots_adjust(left=0.06, right=0.9, top=0.7, bottom=0.15)
    style(ax, fig, t, "GB")
    ax.plot(tt, ram, color=t["s1"], linewidth=2, label="RAM in use (working set)")
    ax.plot(tt, card, color=t["s2"], linewidth=2, label="card memory in use")
    ax.set_xlim(0, tt[-1])
    ax.set_ylim(0, 34)
    ax.set_xlabel("seconds since launch", color=t["muted"], fontsize=9)
    ref(ax, t, 32, "PC RAM 32")
    ref(ax, t, 12, "card 12")
    ax.text(tt[-1] * 1.01, ram[-1], f"RAM {ram[-1]:.1f}", color=t["ink2"], fontsize=8, va="center")
    ax.text(tt[-1] * 1.01, card[-1], f"card {card[-1]:.1f}", color=t["ink2"], fontsize=8, va="center")
    leg = ax.legend(loc="lower left", bbox_to_anchor=(0, 1.0), ncol=2, frameon=False, fontsize=8)
    for tx in leg.get_texts():
        tx.set_color(t["ink2"])
    title(fig, t, "Memory in the same run",
          "Memory bounds this PC: buffer copies on the card took it from about 4 fps to 20.")
    fig.savefig(f"{OUT}/memory-{name}.svg", facecolor=t["surface"])
    preview(fig, f"memory-{name}")
    plt.close(fig)


def tracker(t, name):
    data = sorted(TRACKER, key=lambda p: p[1])
    fig, ax = plt.subplots(figsize=(9, 3.9))
    fig.subplots_adjust(left=0.17, right=0.95, top=0.76, bottom=0.06)
    style(ax, fig, t)
    ax.grid(axis="y", visible=False)
    ax.grid(axis="x", color=t["grid"], linewidth=0.8)
    ax.spines["bottom"].set_visible(False)
    ax.spines["left"].set_visible(True)
    ax.spines["left"].set_color(t["axis"])
    names = [n for n, _ in data]
    vals = [v for _, v in data]
    cols = [t["s1"] if n == "Kitty6" else t["other"] for n in names]
    ax.barh(names, vals, color=cols, height=0.62, edgecolor=t["surface"], linewidth=2)
    for i, v in enumerate(vals):
        ax.text(v + 0.3, i, f"{v:g}", va="center", color=t["ink2"], fontsize=8)
    for lbl in ax.get_yticklabels():
        lbl.set_color(t["ink"] if lbl.get_text() == "Kitty6" else t["ink2"])
    ax.set_xlim(0, 32)
    title(fig, t, "Reported frame rates (26 Nov 2026)",
          "Each project's own figure as the GTA VI PC Race tracker lists it: different PCs, scenes and "
          "methods. Kitty6's is from scripted runs.")
    fig.savefig(f"{OUT}/tracker-{name}.svg", facecolor=t["surface"])
    preview(fig, f"tracker-{name}")
    plt.close(fig)


def main():
    os.makedirs(OUT, exist_ok=True)
    data = runs()
    for n, m in data:
        print(f"{n:14s} {m:5.1f}")
    r = rows(RUN)
    for name, t in THEMES.items():
        progress(t, name, data)
        one_run(t, name, r)
        memory(t, name, r)
        tracker(t, name)
    print("charts in", OUT)


if __name__ == "__main__":
    main()
