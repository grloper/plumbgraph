#!/usr/bin/env python3
"""Render assets/social_preview.png (1280x640) with headless Chrome.

The terminal card shows the finding headlines (lines starting with a severity) taken
verbatim from docs/demo/verify.txt, which scripts/render-demo.py captured from a real run.
"""
import html, pathlib, shutil, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
logo = (ROOT / "assets" / "logo.svg").read_text()
lines = [l for l in (ROOT / "docs/demo/verify.txt").read_text().splitlines() if l.startswith(("HIGH", "MEDIUM", "LOW"))]
verdict = [l for l in (ROOT / "docs/demo/verify.txt").read_text().splitlines() if l.startswith("verify:")]
def cls(l): return "m" if l.startswith("MEDIUM") else "h" if l.startswith("HIGH") else "l"
rows = "".join(f'<div class="{cls(l)}">{html.escape(l[:78] + ("…" if len(l) > 78 else ""))}</div>' for l in lines)
v = html.escape(verdict[0][:78]) if verdict else ""
page = f"""<!doctype html><meta charset=utf-8><style>
*{{box-sizing:border-box;margin:0}}
body{{width:1280px;height:640px;font-family:'Inter','Geist',sans-serif;color:#E2E8F0;overflow:hidden;
background:radial-gradient(900px 500px at 85% 10%,#16305a 0%,transparent 60%),radial-gradient(700px 500px at 0% 100%,#0b3b3c 0%,transparent 60%),#0A1020;position:relative}}
.logo{{position:absolute;left:84px;top:80px;width:132px;height:132px}} .logo svg{{width:132px;height:132px}}
h1{{position:absolute;left:84px;top:236px;font-size:84px;font-weight:800;letter-spacing:-2px;color:#F8FAFC}}
.t{{position:absolute;left:88px;top:340px;font-size:29px;font-weight:600;color:#5EEAD4}}
.s{{position:absolute;left:88px;top:394px;width:470px;font-size:21px;line-height:1.45;color:#94A3B8}}
.c{{position:absolute;left:88px;top:508px;display:flex;gap:12px}}
.c span{{font-size:17px;padding:8px 16px;border-radius:999px;border:1px solid #2B3B5C;background:#101B33;color:#CBD5E1}}
.term{{position:absolute;left:600px;top:150px;width:640px;border-radius:16px;background:#0B1220;border:1px solid #243049;box-shadow:0 30px 80px rgba(0,0,0,.5);overflow:hidden}}
.bar{{height:40px;background:#0F1930;display:flex;align-items:center;gap:8px;padding-left:16px}}
.bar i{{width:12px;height:12px;border-radius:50%;display:block}}
.body{{padding:18px 20px 22px;font:12.2px/1.8 'Geist Mono','DejaVu Sans Mono',monospace;white-space:nowrap}}
.p{{color:#34D399}} .m{{color:#FBBF24;font-weight:600}} .h{{color:#F87171;font-weight:600}} .v{{color:#F87171;font-weight:700;margin-top:10px}}
.cap{{position:absolute;left:600px;top:480px;font-size:14px;color:#64748B}}
</style>
<div class=logo>{logo}</div><h1>Plumbgraph</h1>
<div class=t>Know your code. Verify the change.</div>
<div class=s>Code intelligence and a pre-submit gate for AI coding agents. Local, open source, MCP-native.</div>
<div class=c><span>dead code</span><span>hallucinated deps</span><span>weakened tests</span><span>MCP</span></div>
<div class=term><div class=bar><i style="background:#F87171"></i><i style="background:#FBBF24"></i><i style="background:#34D399"></i></div>
<div class=body><div><span class=p>$</span> plumb verify --run --online</div>{rows}<div class=v>{v}</div></div></div>
<div class=cap>Real output from examples/demo-app (see docs/demo)</div>"""
tmp = pathlib.Path(tempfile.mkdtemp()) / "p.html"
tmp.write_text(page)
chrome = next(c for c in ("google-chrome", "chromium", "chromium-browser") if shutil.which(c))
subprocess.run([chrome, "--headless", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--window-size=1280,640",
                f"--screenshot={ROOT/'assets'/'social_preview.png'}", f"file://{tmp}"], check=True, capture_output=True)
print("wrote assets/social_preview.png")
