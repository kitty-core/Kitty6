# Draws a flame graph (a standalone SVG, no scripts or external tools) from a
# folded-stack file, e.g. kitty6-prof --folded out.folded. Each line is
# "frame;frame;... count", root first; kitty6-prof's first frame is
# "thread [role]" and samples that wait end in a "[wait: kind]" frame.
#
#   python tools/charts/flame.py prof.folded prof.svg
#   python tools/charts/flame.py prof.folded prof.svg --title "S-tl, 50 Hz" --color role
#   python tools/charts/flame.py prof.folded prof.svg --per-role flames/
#
# Options:
#   --title T        heading (default: the input file's name)
#   --color MODE     module (default): hue by crate / module, the title's code,
#                    the system's and the card driver's each their own family;
#                    role: hue by thread role
#   --per-role DIR   also one SVG per role in DIR (<role>.svg)
#   --width PX       picture width (default 1600)
#   --min-px PX      frames narrower than this are left out (default 0.4)
#   --no-waits       drop the "[wait: kind]" frames
# Hovering a frame shows its name, samples, share and role (SVG <title>).
import argparse
import hashlib
import html
import os
import re
import sys

ROW = 17
FONT = 11
CHAR = 6.4  # average glyph width at FONT px, for fitting labels
PAD = 10
HEAD = 56

SYSTEM = {"ntdll", "kernelbase", "kernel32", "win32u", "user32", "ucrtbase", "vcruntime140", "vcruntime140_1",
          "msvcp140", "combase", "rpcrt4", "sechost", "advapi32", "ws2_32", "mswsock", "gdi32", "gdi32full", "msvcrt"}
CARD = {"d3d12", "d3d12core", "d3d11", "dxgi", "dxcore", "d3d12sdklayers", "nvwgf2umx", "nvldumdx", "nvapi64",
        "nvoglv64", "amdxc64", "amdxx64", "amdihk64", "igd12umd64", "igdumdim64", "igc64"}
WAIT_COLORS = {"lock": "#c9a3a3", "event": "#a9b8cf", "fence": "#9fc7b4", "sleep": "#cfcfcf", "idle": "#dedede",
               "message": "#c8bedb", "other": "#d6d0c4"}
ROLE_RE = re.compile(r"^(.*) \[(.*)\]$")


def parse(path, waits=True):
    """[(frames, count)] from a folded file."""
    out = []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line.strip():
                continue
            stack, _, n = line.rpartition(" ")
            try:
                count = int(float(n))
            except ValueError:
                continue
            frames = stack.split(";")
            if not waits and frames and frames[-1].startswith("[wait: "):
                frames = frames[:-1]
            if frames and count > 0:
                out.append((frames, count))
    return out


def role_of(first_frame):
    m = ROLE_RE.match(first_frame)
    return m.group(2) if m else first_frame


class Node:
    __slots__ = ("name", "count", "children", "role")

    def __init__(self, name, role=None):
        self.name = name
        self.count = 0
        self.children = {}
        self.role = role


def build(stacks):
    root = Node("all")
    for frames, n in stacks:
        role = role_of(frames[0])
        root.count += n
        node = root
        for f in frames:
            child = node.children.get(f)
            if child is None:
                child = node.children[f] = Node(f, role)
            child.count += n
            node = child
    return root


def module_of(name):
    """The family of a frame: (kind, key). kind: wait, thread, system, card,
    title, ours."""
    if name.startswith("[wait: "):
        return "wait", name[7:-1]
    if ROLE_RE.match(name):
        return "thread", name
    if "!" in name and not re.search(r"[: <]", name.split("!", 1)[0]):
        mod = name.split("!", 1)[0].lower()
    elif "+0x" in name:
        mod = name.split("+0x", 1)[0].lower().rsplit(".", 1)[0]
        if mod not in SYSTEM and mod not in CARD:
            return "title", mod
    elif name.startswith("0x"):
        return "title", "?"
    else:
        # A Rust or C++ path: its crate / namespace.
        return "ours", name.split("::", 1)[0]
    if mod in SYSTEM:
        return "system", mod
    if mod in CARD:
        return "card", mod
    return "ours", mod


def _h(key, salt=""):
    return int(hashlib.md5((salt + key).encode()).hexdigest()[:8], 16)


def hsl(h, s, l):
    import colorsys
    r, g, b = colorsys.hls_to_rgb(h / 360.0, l / 100.0, s / 100.0)
    return "#%02x%02x%02x" % (round(r * 255), round(g * 255), round(b * 255))


def color(node, mode):
    kind, key = module_of(node.name)
    if kind == "wait":
        return WAIT_COLORS.get(key, "#d6d0c4")
    jitter = _h(node.name) % 10
    if mode == "role" and kind != "thread":
        return hsl(_h(node.role or "", "role") % 360, 55, 66 + jitter / 2)
    if kind == "thread":
        return hsl(_h(node.role or "", "role") % 360, 45, 55)
    if kind == "system":
        return hsl(205 + _h(key) % 20, 30, 74 + jitter / 2)
    if kind == "card":
        return hsl(150 + _h(key) % 25, 45, 62 + jitter / 2)
    if kind == "title":
        return hsl(275 + _h(key) % 30, 40, 70 + jitter / 2)
    # Ours: warm hues by crate.
    return hsl(8 + _h(key) % 45, 75, 60 + jitter)


def depth_of(node):
    return 1 + max((depth_of(c) for c in node.children.values()), default=0)


def render(root, title, width=1600, min_px=0.4, mode="module", subtitle=""):
    depth = depth_of(root)
    total = max(root.count, 1)
    height = HEAD + depth * ROW + PAD * 2 + 18
    scale = (width - 2 * PAD) / total
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" '
        f'font-family="Segoe UI, Arial, sans-serif" font-size="{FONT}">',
        "<style>rect.f{stroke:#fff;stroke-width:0.5}g.n:hover rect.f{stroke:#000;stroke-width:1}"
        "text.l{pointer-events:none;fill:#111}</style>",
        f'<rect width="{width}" height="{height}" fill="#fcfcfb"/>',
        f'<text x="{PAD}" y="24" font-size="17" font-weight="600" fill="#0b0b0b">{html.escape(title)}</text>',
        f'<text x="{PAD}" y="44" fill="#52514e">{html.escape(subtitle)}</text>',
    ]
    base = height - PAD - 18

    def draw(node, x, level):
        w = node.count * scale
        if w < min_px:
            return
        y = base - (level + 1) * ROW
        share = 100.0 * node.count / total
        tip = f"{node.name}\n{node.count} samples, {share:.2f}%"
        if node.role and node is not root:
            tip += f"\nrole: {node.role}"
        out.append(f'<g class="n"><title>{html.escape(tip)}</title>'
                   f'<rect class="f" x="{x:.2f}" y="{y}" width="{w:.2f}" height="{ROW - 1}" rx="1.5" '
                   f'fill="{"#e8e6df" if node is root else color(node, mode)}"/>')
        fit = int((w - 6) / CHAR)
        if fit >= 3:
            label = node.name if len(node.name) <= fit else node.name[: fit - 2] + ".."
            out.append(f'<text class="l" x="{x + 3:.2f}" y="{y + ROW - 5}">{html.escape(label)}</text>')
        out.append("</g>")
        cx = x
        for name in sorted(node.children):
            child = node.children[name]
            draw(child, cx, level + 1)
            cx += child.count * scale

    draw(root, PAD, 0)
    legend = ("colors: warm = our code by crate, blue-grey = system, green = card driver / D3D12, "
              "violet = title code; wait frames: red lock, blue event, green fence, grey sleep/idle"
              if mode == "module" else "colors: by thread role; wait frames: red lock, blue event, green fence, grey sleep/idle")
    out.append(f'<text x="{PAD}" y="{height - 8}" fill="#898781" font-size="10">{html.escape(legend)}</text>')
    out.append("</svg>")
    return "\n".join(out)


def safe(name):
    return re.sub(r"[^A-Za-z0-9._-]+", "_", name).strip("_") or "role"


def main(argv):
    ap = argparse.ArgumentParser(description="folded stacks -> standalone SVG flame graph")
    ap.add_argument("folded")
    ap.add_argument("svg")
    ap.add_argument("--title")
    ap.add_argument("--color", choices=["module", "role"], default="module")
    ap.add_argument("--per-role", metavar="DIR")
    ap.add_argument("--width", type=int, default=1600)
    ap.add_argument("--min-px", type=float, default=0.4)
    ap.add_argument("--no-waits", action="store_true")
    a = ap.parse_args(argv)
    stacks = parse(a.folded, waits=not a.no_waits)
    if not stacks:
        sys.exit(f"{a.folded}: no stacks")
    title = a.title or os.path.basename(a.folded)
    root = build(stacks)
    roles = sorted({role_of(f[0]) for f, _ in stacks})
    sub = f"{root.count} samples, {len(roles)} roles; width = share of all thread samples"
    with open(a.svg, "w", encoding="utf-8") as f:
        f.write(render(root, title, a.width, a.min_px, a.color, sub))
    print(f"written {a.svg}")
    if a.per_role:
        os.makedirs(a.per_role, exist_ok=True)
        for role in roles:
            part = [(fr, n) for fr, n in stacks if role_of(fr[0]) == role]
            r = build(part)
            path = os.path.join(a.per_role, safe(role) + ".svg")
            with open(path, "w", encoding="utf-8") as f:
                f.write(render(r, f"{title}: {role}", a.width, a.min_px, a.color,
                               f"{r.count} samples of role {role}; width = share of this role's samples"))
            print(f"written {path}")


if __name__ == "__main__":
    main(sys.argv[1:])
