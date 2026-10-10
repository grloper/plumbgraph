# Validation report

End-to-end validation of `plumb` run on 2026-10-10 against real repositories. Everything below was run with the
binary built from `main` after PRs #14-#18 (all merged, CI green). Raw logs (stdout/stderr, timing, max RSS per command)
are in [`docs/validation/<repo>/`](validation/); `summary.tsv` in each folder has rc / wall time / peak RSS / output bytes.
Box: 16 GB RAM Linux, release build, tier-0 only (no SCIP indexes, no network registry lookups unless stated).
The strategy this was built around is in [`STRATEGY.md`](STRATEGY.md).

## Samples

| repo | lang | files indexed | symbols | edges | index | peak RSS | map | dead-code (app mode) | check-deps (offline) |
|---|---|---|---|---|---|---|---|---|---|
| requests | py | 37 | 911 | 3.1k | 0.11 s | 26 MB | 26 ms | 1 | 9 (low) |
| express | js | 141 | 586 | 2.1k | 0.14 s | 28 MB | 36 ms | 0 | 26 (23 low, 3 medium) |
| ripgrep | rs | 110 | 4.1k | 42k | 0.41 s | 76 MB | 91 ms | 47 (5 high) | 3 |
| cobra | go | 36 | 674 | 3.4k | 0.13 s | 30 MB | 31 ms | 43 (0 high; 0 with `--lib`) | 0 |
| gson | java | 264 | 4.2k | 42k | 0.39 s | 62 MB | 92 ms | 24 (0 high) | 0 |
| Newtonsoft.Json | cs | 951 | 9.3k | 138k | 1.3 s | 167 MB | 0.23 s | 78 (11 high) | 0 |
| Wraith (own project) | mixed | 34 | 627 | 2.5k | 0.10 s | 27 MB | 27 ms | 1 | 1 |
| classroom-archiver (own) | py | 62 | 364 | 0.9k | 0.09 s | 18 MB | 24 ms | 3 | 0 |
| VS Code | ts | 14,626 of 20,339 tracked | 203k | 6.0M | 60 s | 3.9 GB | 8.5 s | 969 (59 high) | 203 KB output |
| kubernetes | go | 13,558 of 31,421 tracked (rest are yaml/json/md/non-code) | 145k | 5.3M | 41 s | 3.3 GB | 7.0 s | 4,996 (28 high) | 4 |

Second index run (nothing changed): 9-340 ms on every repo.

## Pass / fail matrix

| area | result | evidence |
|---|---|---|
| All commands exit cleanly on 10 repos (index, map, dead-code, check-deps, weakening, verify, doctor, impact) | PASS, no panics, no OOM | `validation/*/summary.tsv` |
| Kubernetes scale (was: 640 s index, 5.7 GB DB, map/dead-code/check-deps/verify/impact OOM-killed at ~11 GB) | PASS now: index 41 s, DB 0.8 GB, every command 6-13 s, peak RSS 2.3-3.3 GB | `validation/k8s/`; PR #14 (edge dedup + ambiguity cap), #16 (Go/Java import resolution was O(imports x files)) |
| Output bounded (Newtonsoft check-deps was 2.1 MB, k8s verify 4.8 MB) | PASS: `--max-findings` (default 200, 0 = all) on dead-code, check-deps, verify; totals, verdict and exit code still cover everything; JSON gets a `truncated` object | PR #16, #18; CLI tests |
| check-deps false positives | PASS on the repos seen: Newtonsoft 2904 -> 0, express 96 -> 0 unresolved-import, requests py2 guarded imports now low | PR #14 |
| dead-code false positives from import aliases (`use x as y`, incl. `pub use` re-exports used in other files) | PASS on ripgrep (HIGH FPs 2 -> 0; findings 75 -> 47) | PR #14, #18 |
| Test-weakening detection, seeded commits (4 skip + 4 loosened assertion + 4 deleted test + 3 benign per repo) | PASS: python, js, rust, go, c# 12/12; java skip 4/4, delete 4/4 (loosened-assertion seeds did not apply to gson's assertion style, so 0 tested); benign changes flagged 0/3 per repo. Before: Go 0/12, Java 0/8, C# 0/12 silently | PR #15; `validation/<repo>/weakening_seeded.json` |
| Go method names (`Context.JSON` vs bare `JSON`) | PASS, unit tested | PR #17 |
| MCP `repo_map` token cost | PASS: default returns only the budgeted text (the structured list was ~4x the budget); `format=json` opt-in. Note: MCP clients still receive the text twice (content + structuredContent) | PR #17 |
| Closed stdout pipe (`plumb ... | head`) | PASS: exits 141 quietly instead of a panic | PR #16, CLI test |
| MCP with the official TypeScript SDK client (`scripts/mcp-sdk-check.mjs`) on requests and kubernetes | PASS: initialize, 13 tools, 3 resources, tool calls; payloads bounded (k8s dead_code 72 KB `truncated:true`, verify 152 KB) | `validation/mcp/` |
| Edge cases: empty repo, random binary named `.py`/`.rs`, non-UTF-8 and BOM sources, symlinks (file, loop, dangling, out of tree), 3 MB source file, submodule | PASS: no panics, rc 0; symlinks not followed; files over 2 MiB skipped with a warning | `validation/` (see "Edge cases" below) |
| `plumb impact` in a repo with no commits | clear error, exit 2 (acceptable) | |
| Real agent CLI driving the MCP server | NOT RUN: installable (e.g. `@anthropic-ai/claude-code`) but needs an interactive login/API key that was not available | |
| With/without repo-map experiment on real agent tasks | NOT RUN: needs an LLM agent harness and keys. No claim is made about whether the map improves agent outcomes | |

## Dead-code precision (hand-checked samples, not exhaustive)

- ripgrep: 12 sampled before fixes: 2 HIGH FPs (aliases, fixed), several medium FPs that were callees of alias-only functions (fixed by the same change). The 5 remaining HIGH findings were read: `Handle::begin_read`, `read_write_mut`, `TSeq::max_literal_len` have no callers; treated as true positives.
- cobra: 12 sampled, all are public API (app mode treats exported as unused: documented, `--lib` gives 0).
- gson: 12 sampled, all medium; reflective / cross-module entry points that the tool flags with `fp_risk` evidence.
- Wraith 1/1, classroom-archiver 3/3 true positives. requests 1 (string reference in docs config).
- express 0 findings: recall not checked.
- kubernetes / VS Code: counts only, not sampled. Treat the 4,996 / 969 findings as a worklist for review, not as deletions.

## Edge cases (all run through index, dead-code, map, check-deps, verify, impact)

empty git repo: 0 files, all rc 0. Binary 300 KB `blob.py`: indexed as Python with a syntax-error warning (it creates ~34k bogus reference rows: harmless but wasteful). Non-UTF-8 and BOM Python: parsed. Symlinks (to file, to `.` loop, to `/etc/passwd`, dangling): skipped, not followed. 3.2 MB Python file: skipped "too large". Submodule checkout: indexed as part of the tree.

## What does not work / known limits

- Memory: indexing holds the whole graph in memory. Peak RSS was 3.3 GB (kubernetes, 13.5k files / 6M reference rows) and 3.9 GB (VS Code, 14.6k files / 7.3M rows); analysis commands load about 2.3 GB. A repo several times larger will need more RAM than most laptops have; there is no size guard or streaming mode yet, so an OOM is still possible beyond ~30-40k source files on a 16 GB machine.
- Analysis is name-based (tree-sitter, no types). Names with more than 32 same-named definitions get no name-based edges, and such symbols are never reported as dead (this removes a class of FPs and hides some real dead code on huge repos).
- Dead-code in app mode flags exported symbols of libraries unless run with `--lib` (a note is printed; auto-detecting library crates is not implemented).
- Member calls (`x.name()`) create weak edges to every same-named method: dead-code recall is reduced, impact is over-approximate.
- Rust `pub use x as y` now keeps `x` alive even when `y` is never used anywhere (deliberate, conservative). Aliases through macros are not followed.
- check-deps offline cannot know about registry-only packages; online mode was tested only on small repos (requests, express, ripgrep).
- C# `using` namespaces are not mapped to files (they are reported as external, never unresolved).
- Weakening detection is regex/AST-pattern based on the diff: it catches skips, deleted tests and common assertion loosening, not semantic weakening (e.g. changing an expected value).
- MCP `prompts/list` is not implemented (returns -32601; harmless).
- Only ~10 repos in 6 languages were exercised; Kotlin, C/C++, PHP, Ruby, Swift are untested.
