#!/usr/bin/env python3
"""Compare two `plumb --json dead-code` outputs (e.g. --no-scip vs. with SCIP).

usage: scripts/compare-dead-code.py NAME_BASED.json SCIP.json
Prints the summary of each run and every finding that is new, gone, or changed (confidence/source).
Use --include-low when producing the inputs, otherwise low findings are hidden.
"""
import json
import sys


def load(p):
    d = json.load(open(p))
    return d, {(f["symbol"], f["file"], f["line"]): f for f in d["findings"]}


(a, ak), (b, bk) = load(sys.argv[1]), load(sys.argv[2])
print("A", a["summary"])
print("B", b["summary"], {k: v for k, v in (b.get("scip") or {}).items() if isinstance(v, int)})
for k in sorted(set(bk) - set(ak)):
    print("  only in B ", bk[k]["level"], bk[k]["confidence"], k, bk[k]["source"])
for k in sorted(set(ak) - set(bk)):
    print("  only in A ", ak[k]["level"], ak[k]["confidence"], k, ak[k]["source"])
for k in sorted(set(ak) & set(bk)):
    x, y = ak[k], bk[k]
    mark = "=" if (x["confidence"], x["source"]) == (y["confidence"], y["source"]) else "~"
    print(f"  {mark} {k} {x['confidence']} {x['level']} {x['source']} -> {y['confidence']} {y['level']} {y['source']}")
