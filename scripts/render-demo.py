#!/usr/bin/env python3
"""Render README demo images from REAL runs of `plumb` against examples/demo-app.

Nothing here is typed by hand: every output line is captured from the process and
written verbatim to docs/demo/*.txt; the SVGs are just a terminal-style rendering of
those captures (animated SVG for the README, plus a static PNG of the final frame).

Needs: a built `plumb` (cargo build --release), semgrep, ast-grep and scip-python on PATH,
network for `--online`, and Chrome/Chromium for the PNGs.
Usage: scripts/render-demo.py
"""
import html, os, pathlib, shutil, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "demo"
PLUMB = ROOT / "target" / "release" / "plumb"

SCENES = {
    "verify": ["plumb verify --run --online --baseline plumb-baseline.json"],
    "map-impact": ["plumb map --tokens 300", "plumb impact format_user"],
    "scip": ["plumb enrich", "plumb dead-code ."],
}


def run_scene(cmds, cwd, env):
    frames = []
    for c in cmds:
        argv = c.split()
        p = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, timeout=300)
        out = (p.stdout + p.stderr).rstrip("\n")
        frames.append((c, out.splitlines(), p.returncode))
    return frames


def classify(line):
    s = line.lstrip()
    for tag, cls in (("HIGH", "hi"), ("MEDIUM", "me"), ("LOW", "lo")):
        if line.startswith(tag):
            return "sev-" + cls
    if s.startswith(("why:", "risk:")):
        return "dim"
    if line.startswith(("verify: FAIL", "verify: REPORT")):
        return "bad"
    if line.startswith("verify: PASS"):
        return "ok"
    if line.startswith("note:"):
        return "dim"
    return "fg"


def svg(frames, animated=True, cycle=14.0):
    cw, lh, pad_x, top = 7.85, 19, 22, 52
    rows = []  # (kind, text, appear_fraction)
    t = 0.4
    step = 0.09
    for cmd, lines, _ in frames:
        rows.append(("cmd", cmd, t)); t += 0.9
        for ln in lines:
            rows.append((classify(ln), ln, t)); t += step
        t += 0.5
    total = t + 3.0
    width = int(max(len(r[1]) + 4 for r in rows) * cw + pad_x * 2)
    width = max(width, 640)
    height = top + len(rows) * lh + 26 + len(frames) * 4
    css = [
        "text{font-family:'JetBrains Mono','Fira Code','DejaVu Sans Mono','Menlo','Consolas',monospace;font-size:13px;white-space:pre}",
        ".fg{fill:#E2E8F0}.dim{fill:#7C8BA5}.cmd{fill:#F8FAFC}.prompt{fill:#34D399}",
        ".sev-hi{fill:#F87171;font-weight:700}.sev-me{fill:#FBBF24;font-weight:700}.sev-lo{fill:#7DD3FC;font-weight:700}",
        ".bad{fill:#F87171;font-weight:700}.ok{fill:#34D399;font-weight:700}",
    ]
    body = []
    y = top
    kf = []
    for i, (kind, text, at) in enumerate(rows):
        p0 = at / total * 100
        anim = ""
        if animated:
            kf.append(f"@keyframes k{i}{{0%{{opacity:0}}{max(p0-0.01,0):.2f}%{{opacity:0}}{p0:.2f}%{{opacity:1}}96%{{opacity:1}}100%{{opacity:0}}}}")
            anim = f' style="opacity:0;animation:k{i} {total:.1f}s linear infinite"'
        esc = html.escape(text)
        if kind == "cmd":
            body.append(f'<text xml:space="preserve" x="{pad_x}" y="{y}"{anim}><tspan class="prompt">$ </tspan><tspan class="cmd">{esc}</tspan></text>')
        else:
            body.append(f'<text xml:space="preserve" x="{pad_x}" y="{y}" class="{kind}"{anim}>{esc}</text>')
        y += lh + (4 if kind != "cmd" and False else 0)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="Terminal recording of real plumb output">
<style>{"".join(css)}{"".join(kf)}</style>
<rect width="{width}" height="{height}" rx="12" fill="#0B1220"/>
<rect x="0.5" y="0.5" width="{width-1}" height="{height-1}" rx="11.5" fill="none" stroke="#243049"/>
<circle cx="24" cy="24" r="6" fill="#F87171"/><circle cx="44" cy="24" r="6" fill="#FBBF24"/><circle cx="64" cy="24" r="6" fill="#34D399"/>
<text x="{width/2}" y="28" text-anchor="middle" class="dim" style="font-size:12px">plumb: real output (examples/demo-app)</text>
{"".join(body)}
</svg>'''


def render_png(svg_path, png_path, svg_text):
    """Rasterise with headless Chrome (honours fonts/whitespace); no cairosvg fallback on purpose."""
    import re
    chrome = next((c for c in ("google-chrome", "chromium", "chromium-browser") if shutil.which(c)), None)
    if not chrome:
        print("note: no chrome/chromium found; skipping PNG", file=sys.stderr)
        return
    m = re.search(r'width="(\d+)" height="(\d+)"', svg_text)
    w, h = int(m.group(1)), int(m.group(2))
    subprocess.run([chrome, "--headless", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
                    "--force-device-scale-factor=2", f"--window-size={w},{h}",
                    f"--screenshot={png_path}", f"file://{svg_path}"],
                   check=True, capture_output=True)


def main():
    if not PLUMB.exists():
        sys.exit("build first: cargo build --release")
    tmp = pathlib.Path(tempfile.mkdtemp())
    bindir = tmp / "bin"; bindir.mkdir()
    (bindir / "plumb").symlink_to(PLUMB)
    work = pathlib.Path("/tmp/demo-app")  # fixed path so absolute paths in the output are stable
    shutil.rmtree(work, ignore_errors=True)
    shutil.copytree(ROOT / "examples" / "demo-app", work, ignore=shutil.ignore_patterns(".plumbgraph", "plumb-baseline.json"))
    subprocess.run(["git", "init", "-q"], cwd=work, check=True)
    subprocess.run(["git", "add", "-A"], cwd=work, check=True)
    subprocess.run(["git", "-c", "user.name=demo", "-c", "user.email=demo@example.invalid", "commit", "-qm", "demo"], cwd=work, check=True)
    env = dict(os.environ, PATH=f"{bindir}:{os.environ['PATH']}", NO_COLOR="1")
    OUT.mkdir(parents=True, exist_ok=True)
    for name, cmds in SCENES.items():
        frames = run_scene(cmds, work, env)
        (OUT / f"{name}.txt").write_text("".join(f"$ {c}\n" + "\n".join(l) + "\n" for c, l, _ in frames))
        (OUT / f"{name}.svg").write_text(svg(frames, animated=True))
        static = svg(frames, animated=False)
        (OUT / f"{name}-static.svg").write_text(static)
        render_png(OUT / f"{name}-static.svg", OUT / f"{name}.png", static)
        print("rendered", name, [rc for _, _, rc in frames])
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
