#!/usr/bin/env python3
"""Measure plumb on real repositories. Nothing here is invented: every number is printed
from a command that ran. Usage:

  scripts/bench.py --plumb target/release/plumb --out docs/bench/results.json NAME=PATH [NAME=PATH ...]

Each repository is copied to a scratch directory first (node_modules/target/.git/dist are skipped), so
the originals are untouched. Measured per repository:
  cold index, no-change re-index (median of 3), one-file-change re-index, `map` at 1000/2000 tokens,
  dead-code findings, `impact` on the top-ranked symbol, `verify` (offline, nothing executed),
  and recall/false-positive rate on SEEDED dead code (synthetic, uniquely named functions injected
  next to live ones; this is an easy upper bound, not a real-world recall).
"""
import argparse, json, os, platform, random, re, shutil, statistics, subprocess, sys, tempfile, time
from pathlib import Path

SKIP = {"node_modules", "target", ".git", "dist", ".plumbgraph", "venv", ".venv", "__pycache__", "build"}


def copy_tree(src, dst):
    for root, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if d not in SKIP]
        rel = os.path.relpath(root, src)
        os.makedirs(os.path.join(dst, rel), exist_ok=True)
        for f in files:
            p = os.path.join(root, f)
            if os.path.islink(p):
                continue
            try:
                if os.path.getsize(p) < 2_000_000:
                    shutil.copy2(p, os.path.join(dst, rel, f))
            except OSError:
                pass


def run(plumb, args, cwd=None):
    t = time.perf_counter()
    p = subprocess.run([plumb] + args, capture_output=True, text=True, cwd=cwd)
    return time.perf_counter() - t, p


def jrun(plumb, args):
    dt, p = run(plumb, args)
    try:
        return dt, p.returncode, json.loads(p.stdout)
    except Exception:
        return dt, p.returncode, None


def count_langs(root):
    c = {}
    for r, ds, fs in os.walk(root):
        ds[:] = [d for d in ds if d not in SKIP]
        for f in fs:
            e = os.path.splitext(f)[1]
            c[e] = c.get(e, 0) + 1
    return c


def source_files(root, exts, avoid_tests=True):
    out = []
    for r, ds, fs in os.walk(root):
        ds[:] = [d for d in ds if d not in SKIP]
        for f in fs:
            p = os.path.join(r, f)
            rel = os.path.relpath(p, root)
            if os.path.splitext(f)[1] in exts and not (avoid_tests and re.search(r"(test|spec|example|bench|fixture)", rel, re.I)):
                out.append(p)
    return sorted(out)


def seed(root, n, rng):
    """Inject n dead + n live synthetic functions. Returns (dead_names, live_names, language)."""
    langs = count_langs(root)
    best = max([(langs.get(e, 0), e) for e in (".py", ".js", ".rs", ".go", ".java")], default=(0, ""))[1]
    dead = [f"plumb_seed_dead_{i}" for i in range(n)]
    live = [f"plumb_seed_live_{i}" for i in range(n)]
    if best == ".py":
        files = source_files(root, {".py"})
        for i, d in enumerate(dead):
            with open(rng.choice(files), "a") as f:
                f.write(f"\n\ndef {d}():\n    return {i}\n")
        f = rng.choice(files)
        with open(f, "a") as fh:
            for i, l in enumerate(live):
                fh.write(f"\n\ndef {l}():\n    return {i}\n\n{l}()\n")
    elif best == ".js":
        files = source_files(root, {".js"})
        for i, d in enumerate(dead):
            with open(rng.choice(files), "a") as f:
                f.write(f"\nfunction {d}() {{ return {i}; }}\n")
        with open(rng.choice(files), "a") as fh:
            for i, l in enumerate(live):
                fh.write(f"\nfunction {l}() {{ return {i}; }}\n{l}();\n")
    elif best == ".rs":
        files = source_files(root, {".rs"})
        for i, d in enumerate(dead):
            with open(rng.choice(files), "a") as f:
                f.write(f"\nfn {d}() -> u32 {{ {i} }}\n")
        # live functions live in a new binary target next to an existing crate's src/
        crate_src = next((os.path.dirname(f) for f in files if f.endswith("main.rs") or f.endswith("lib.rs")), os.path.dirname(files[0]))
        os.makedirs(os.path.join(crate_src, "bin"), exist_ok=True)
        with open(os.path.join(crate_src, "bin", "plumb_seed.rs"), "w") as fh:
            for i, l in enumerate(live):
                fh.write(f"fn {l}() -> u32 {{ {i} }}\n")
            fh.write("fn main() {\n" + "".join(f"    let _ = {l}();\n" for l in live) + "}\n")
    elif best == ".go":
        files = source_files(root, {".go"})
        pkgdir = os.path.dirname(files[0])
        pkg = re.search(r"^package (\w+)", open(files[0]).read(), re.M).group(1)
        go_dead = [f"plumbSeedDead{i}" for i in range(n)]
        go_live = [f"plumbSeedLive{i}" for i in range(n)]
        with open(os.path.join(pkgdir, "plumb_seed.go"), "w") as fh:
            fh.write(f"package {pkg}\n\n")
            for i, d in enumerate(go_dead):
                fh.write(f"func {d}() int {{ return {i} }}\n\n")
            for i, l in enumerate(go_live):
                fh.write(f"func {l}() int {{ return {i} }}\n\nvar _ = {l}()\n\n")
        dead, live = go_dead, go_live
    elif best == ".java":
        files = source_files(root, {".java"})
        jd = [f"plumbSeedDead{i}" for i in range(n)]
        jl = [f"plumbSeedLive{i}" for i in range(n)]
        d = os.path.dirname(files[0])
        with open(os.path.join(d, "PlumbSeed.java"), "w") as fh:
            fh.write("class PlumbSeed {\n    public static void main(String[] args) {\n")
            fh.write("".join(f"        {l}();\n" for l in jl) + "    }\n")
            for i, x in enumerate(jd):
                fh.write(f"    private static int {x}() {{ return {i}; }}\n")
            for i, x in enumerate(jl):
                fh.write(f"    private static int {x}() {{ return {i}; }}\n")
            fh.write("}\n")
        dead, live = jd, jl
    else:
        return [], [], None
    return dead, live, best


def bench_repo(plumb, name, src, nseed):
    rng = random.Random(1)
    res = {"name": name}
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.join(tmp, "r")
        copy_tree(src, root)
        res["extensions"] = dict(sorted(count_langs(root).items(), key=lambda kv: -kv[1])[:6])
        db = os.path.join(tmp, "idx.db")
        base = ["--db", db]
        dt, rc, st = jrun(plumb, base + ["--json", "index", root])
        res["cold_index_s"] = round(dt, 3)
        res["index"] = {k: st[k] for k in ("files_seen", "files_parsed", "symbols", "refs", "edges")} if st else None
        warm = []
        for _ in range(3):
            warm.append(run(plumb, base + ["index", root])[0])
        res["noop_reindex_s_median"] = round(statistics.median(warm), 3)
        # touch one file
        files = source_files(root, {".py", ".js", ".rs", ".go", ".java", ".ts"}, avoid_tests=False)
        if files:
            with open(files[0], "a") as f:
                f.write("\n")
            dt, rc, st2 = jrun(plumb, base + ["--json", "index", root])
            res["one_file_changed_reindex_s"] = round(dt, 3)
            res["one_file_changed_parsed"] = st2["files_parsed"] if st2 else None
        for tok in (1000, 2000):
            dt, rc, m = jrun(plumb, base + ["--json", "map", root, "--tokens", str(tok)])
            if m:
                res[f"map_{tok}"] = {"seconds": round(dt, 3), "tokens_estimated": m["tokens_estimated"], "symbols_shown": m["shown_symbols"], "symbols_total": m["total_symbols"], "bytes": len(m["text"].encode())}
                top = m["files"][0]["symbols"] if m["files"] else []
        # impact on the highest-ranked symbol
        dt, rc, m = jrun(plumb, base + ["--json", "map", root, "--tokens", "300"])
        if m and m["files"]:
            s = max((s for f in m["files"] for s in f["symbols"]), key=lambda s: s["rank"])
            dt, rc, imp = jrun(plumb, base + ["--json", "impact", s["name"], "--path", root])
            res["impact_top_symbol"] = {"symbol": s["name"], "seconds": round(dt, 3), "affected": len(imp["affected"]) if imp else None, "tests_to_run": len(imp["tests_to_run"]) if imp else None, "ok": imp is not None}
        dt, rc, dc = jrun(plumb, base + ["--json", "dead-code", root])
        if dc:
            res["dead_code"] = {"seconds": round(dt, 3), **{k: dc["summary"][k] for k in ("total", "high", "medium", "low")}}
        dt, rc, v = jrun(plumb, base + ["--json", "verify", root, "--baseline", os.path.join(tmp, "b.json")])
        if v:
            res["verify"] = {"seconds": round(dt, 3), "exit": rc, "new": len(v["new"]), "executed": v["executed"], "steps": {s["name"]: s["findings"] for s in v["steps"]}}
        # seeded recall
        dead, live, lang = seed(root, nseed, rng)
        if lang:
            dt, rc, dc = jrun(plumb, base + ["--json", "dead-code", root, "--include-low"])
            if dc:
                found = {f.get("symbol") for f in dc["findings"]}
                found_def = {f.get("symbol") for f in dc["findings"] if f["confidence"] >= 0.70}
                def hit(names, s):
                    return sum(1 for n in names if any(x == n or (x or "").endswith("." + n) or (x or "").endswith("::" + n) for x in s))
                res["seeded"] = {"language": lang, "seeded_dead": len(dead), "seeded_live": len(live),
                                 "dead_found_any_confidence": hit(dead, found), "dead_found_default_threshold": hit(dead, found_def),
                                 "live_flagged_as_dead": hit(live, found)}
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--plumb", required=True)
    ap.add_argument("--out")
    ap.add_argument("--seed", type=int, default=20)
    ap.add_argument("repos", nargs="+")
    a = ap.parse_args()
    out = {"date": time.strftime("%Y-%m-%d %H:%M %Z"), "machine": {"cpus": os.cpu_count(), "platform": platform.platform(), "python": platform.python_version()},
           "plumb_version": subprocess.run([a.plumb, "--version"], capture_output=True, text=True).stdout.strip(), "repos": []}
    for spec in a.repos:
        name, path = spec.split("=", 1)
        sha = subprocess.run(["git", "-C", path, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
        r = bench_repo(a.plumb, name, path, a.seed)
        r["commit"] = sha or None
        out["repos"].append(r)
        print(json.dumps(r, indent=1), file=sys.stderr)
    if a.out:
        Path(a.out).write_text(json.dumps(out, indent=1) + "\n")
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
