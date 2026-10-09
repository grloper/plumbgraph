# Evaluation (sanity runs, not benchmarks)

> v0.2 additions (SCIP and diagnostics) are in [the section at the end](#v02-name-based-vs-scip-and-diagnostics-on-wraith-and-classroom-archiver). The v0.1 runs below are unchanged.

**Read this first.** Nothing here is a measured precision/recall. The numbers below are *sanity runs* by the author on two of their own small projects, checked by reading the code with `grep`. Two repositories, a handful of findings, one person: this shows the tool does not obviously fall over and that the specific findings listed were correct. It says nothing about general accuracy, and recall (what the tool misses) was **not** measured on real code.

Date of run: 2026-10-10, commit of Plumbgraph at the time of the initial PR. Network on (registry lookups real). Commands: `plumb index`, `plumb dead-code`, `plumb check-deps`, `plumb weakening`.

## What is actually proven by automated tests

- Golden tests for dead code on small fixtures with known ground truth (Python, JS, TS, Rust) in `crates/core/tests/fixtures` + `crates/core/tests/golden`. The fixtures were written by the author, so they encode the author's expectations; they guard against regressions, they are not independent validation.
- Golden tests for `check-deps` (offline `StaticRegistry`), unit tests for resolution, manifests, registry error handling, weakening diff analysis, MCP protocol round trips, CLI exit codes.
- A synthetic mutation check for `weakening`: skipping/deleting a test in a copy of classroom-archiver is reported (`added-skip`); see CLI/core tests for the encoded versions.

## Sanity run 1: Wraith (Rust, ~45 commits; 34 indexed files: 23 Rust, 11 Python)

| command | result |
|---|---|
| `index` | 627 symbols, 14 338 references, 197 imports, 4 092 edges |
| `dead-code` | 1 finding (medium 0.77): `Region::is_file_backed` (src/maps.rs:48) |
| `check-deps` | 1 finding (low 0.45): `PIL` imported in `scripts/render_brand.py` with no Python manifest (the pip package is `pillow`) |
| `weakening` | no findings against recent history |

Manual check: `is_file_backed` is a `pub fn` that appears nowhere else in the repo, i.e. a correct finding for a binary crate (it would be a *false* positive if the crate were consumed as a library, hence the "exported/public" risk note and medium confidence). The `PIL` finding is real: the script has no declared dependency; its tool-reported confidence is low for that reason.

## Sanity run 2: classroom-archiver (JavaScript + HTML, 30 commits; 61 JS files)

| command | result |
|---|---|
| `index` | 364 symbols, 11 863 references, 206 imports, 1 565 edges |
| `dead-code` | 3 findings (all medium 0.77): `ArchiveDatabase.updateMaterialPath`, `buildSrcShareLink`, `GoogleSession.signOut` |
| `check-deps` | 0 findings; 22 external packages checked, 12 registry lookups, 0 inconclusive |
| `weakening` | no findings against recent history; mutation test (skip a test in a copy) reports `added-skip` |

Manual check: for all three, `grep` finds only the definition in the repository, so they are genuinely unreferenced in the current tree.

## False positives we hit while building and fixed (each now has a test)

- A Rust function called only inside `format!` macro arguments looked unused (macro token trees are now scanned for calls/member access).
- A locally declared Rust module with the same name as a crates.io crate was reported as an external dependency.
- A function referenced only from an inline `<script>` in HTML looked dead (HTML/config text files are now scanned for identifier mentions).
- Python module-level assignments such as `HANDLER = Handler` were flagged.
- tree-sitter-rust 0.23 mis-parsed `&raw` (fixed by upgrading to 0.24).

## Known limitations

- Dead-code analysis is name-based (tier 0). Dynamic dispatch, reflection, macros that generate items, dependency injection, and code outside the indexed files are invisible; such cases lower confidence but can still produce wrong results in either direction.
- Recall was not measured. Findings at `medium` should be verified by a human or agent before deleting anything.
- Python import-name to distribution-name mapping is a built-in table; unknown mappings can yield false "undeclared" findings.
- Semantic tier: see v0.2 section; it is optional and only as good as the index you feed it.

---

# v0.2: name-based vs SCIP, and diagnostics, on Wraith and classroom-archiver

Same caveats as above, plus: **two small repositories, one person, no ground-truth labelling of recall.** Everything below was produced by the commands listed; nothing is extrapolated. Date: 2026-10-10 (Asia/Jerusalem). Network was available but unused by these commands.

Subjects (local clones, run in scratch copies so the originals stay untouched):

| repo | commit | notes |
|---|---|---|
| Wraith | `37bd689c8343dfd46640004f4dc678d7b3304c55` | Rust + Python scripts; 34 indexed files, 627 symbols. Newer than the v0.1 run above, which is why numbers there and here need not match exactly. |
| classroom-archiver | `39513089fe9307369a4d3770b69d318390cde5c4` | JavaScript (+ HTML); 61 indexed files, 364 symbols |

Tools: rust-analyzer `1.99.0 (b940084 2026-09-28)` (`rust-analyzer scip`, 6.1 s wall time, 1.15 MB index), `@sourcegraph/scip-typescript` 0.4.0 (`scip-typescript index --infer-tsconfig`, 1.9 s, 1.16 MB; no `node_modules` installed, so third-party imports are unresolved), `@sourcegraph/scip-python` 0.6.6, ruff and pyright from PyPI, `tsc` from npm `typescript`, clippy 0.1.99.

## SCIP vs name-based dead code

Commands (`--include-low` so nothing is hidden; compare with `scripts/compare-dead-code.py`):

```console
rust-analyzer scip . --output index.scip        # in the Wraith clone
scip-typescript index --infer-tsconfig          # in the classroom-archiver clone
plumb --json dead-code <repo> --include-low --no-scip   > name.json
plumb --json dead-code <repo> --include-low             > scip.json   # index.scip auto-detected
scripts/compare-dead-code.py name.json scip.json
```

| | Wraith name-based | Wraith + SCIP | classroom-archiver name-based | classroom-archiver + SCIP |
|---|---|---|---|---|
| findings (all levels) | 4 (1 medium, 3 low) | 4 (2 medium, 2 low) | 5 (3 medium, 2 low) | 5 (5 medium, 0 low) |
| symbols resolved by the index | | 407 | | 328 |
| name-based edges replaced / SCIP edges added | | 2395 / 1839 | | 1107 / 1238 |
| stale or unmatched index documents | | 0 / 0 | | 0 / 0 |

The set of reported symbols was **identical** with and without SCIP on both repositories (no new finding, none lost). What changed is confidence and `source` on the findings whose symbol the index resolved:

| finding | name-based | with SCIP | manual check (grep of the clone) |
|---|---|---|---|
| Wraith `is_execve` (`src/syscalls.rs:94`, used-only-by-tests) | low 0.63 | medium 0.77, `scip` | only referenced from its own `#[test]` (`src/syscalls.rs:192`): correct |
| classroom-archiver `flattenAttachments` (`web/src/archive/format.js:212`, used-only-by-tests) | low 0.63 | medium 0.77, `scip` | only referenced from `test/web-archive.test.js`: correct |
| classroom-archiver `safeFilename` (`web/src/util/paths.js:25`) | low 0.49 | medium 0.77, `scip` | the `web/` copy is never imported; the same-named function in `src/utils/paths.js` is the one used. Name-based analysis was confused by the collision, SCIP was not: correct |
| Wraith `Region::is_file_backed`, classroom-archiver `ArchiveDatabase.updateMaterialPath`, `GoogleSession.signOut`, `buildSrcShareLink` | medium 0.77 | medium 0.77, `scip` | unchanged (these have no name collisions); same verdict as v0.1's manual check |

So on these two repos the measurable benefit is **confidence**: at the default threshold (>= 0.70) Wraith goes from 1 to 2 findings and classroom-archiver from 3 to 5, and all additions were verified true by hand. It is *not* shown that SCIP finds dead code the name-based pass cannot; recall was not measured, and 2 repositories cannot support a general claim. The toy `a.py` in the automated tests (two classes with a `run` method, only one called) is the case where SCIP does find something name-based analysis cannot, which is why that test exists.

### Failure modes found while measuring (both now have regression tests)

1. **Untyped receivers.** The first implementation replaced *all* name-based edges that pointed at a SCIP-known symbol. On classroom-archiver this reported **18 additional findings that were all false positives** (e.g. `ArchiveDatabase.upsertCourse`, called as `db.upsertCourse(course)` where `db` has no type information, so scip-typescript emits no reference). Fix: replace a name-based edge only when the index resolved an occurrence of that name on that line (`reference_the_index_does_not_resolve_keeps_its_name_based_edge`).
2. **Duplicate symbol strings.** rust-analyzer gives the same symbol string to same-named items in different binary targets of one crate. The first-definition-wins binding made two live Wraith constants (`PAYLOAD`, `detonate::PAGE` in `src/bin/mt_shellcode_sim.rs`) look dead. Fix: bind to the definition in the reference's own document (`duplicate_symbol_strings_bind_within_their_own_document`).

### Python

`scip-python` 0.6.6 produced a usable index for a 14-line toy (`a.py`: two classes with `run`, one called): name-based reports 0 findings, with the index `B` and `B.run` are reported (`source: scip`, medium 0.77). On Wraith's `scripts/` it crashed with a stack trace and wrote a 54-byte (empty) index. So Python support is demonstrated on a toy only.

## Diagnostics

```console
plumb diagnostics <wraith> --run                   # cargo check + clippy (executes the toolchain)
ruff check --output-format json scripts > r.json;  pyright --outputjson scripts > p.json
plumb diagnostics <wraith> --from ruff-json:r.json --from pyright-json:p.json --min-severity info
plumb diagnostics <classroom-archiver> --run --tool tsc   # with a scratch tsconfig.json (allowJs, checkJs), removed afterwards
plumb diagnostics <wraith copy with an injected type error> --lsp "rustup run stable rust-analyzer" --lsp-file src/maps.rs
```

| run | result |
|---|---|
| Wraith `cargo check` + `clippy --workspace --all-targets` (`--run`) | both `ran`, **0 diagnostics** (confirmed by running `cargo clippy` directly: no warnings) |
| Wraith `scripts/` ruff (default config) | 27 diagnostics, all ingested (raw ruff JSON also has 27) |
| Wraith `scripts/` pyright | 18 diagnostics (raw pyright JSON: 18) |
| classroom-archiver `tsc` with a scratch `allowJs`+`checkJs` config | 744 diagnostics (400 TS7006 implicit-any, 63 TS2345, 59 TS2339, ...). This is a plain-JS project that was never written for `checkJs`; the number measures noise from an inappropriate configuration, **not defects**, and is here only to show the adapter copes with volume |
| rust-analyzer via the LSP client, on a scratch copy of Wraith with `fn plumb_injected_error() -> i32 { "not an int" }` appended to `src/maps.rs` | 2 diagnostics (E0308 `mismatched types`, 0.95, plus its related "expected `i32` because of return type" hint, 0.75), 6.8 s. Before the client advertised work-done-progress and `experimental/serverStatus` support it returned **0 diagnostics** in ~3 s because rust-analyzer publishes an empty list first; that is now covered by a fake-server regression test, and confirmed against the real server only on this one injected error |

Not measured: how often a diagnostic is a real defect versus environment noise (so the per-source confidence constants are unvalidated), LSP behaviour for servers other than rust-analyzer, tsc/ruff/pyright against the repositories' own intended configs, baseline diffing (not implemented).


## v1.0 measurements (2026-10-10, commit `3f1257f`, 8 CPUs, release build)

Raw data: [docs/bench/results-2026-10-10.json](bench/results-2026-10-10.json), produced by `scripts/bench.py` on scratch copies (never the originals). Token counts are an estimate (chars/4), not a real tokenizer. Single run per repo (no-op reindex is a median); wall-clock on one machine.

| repo | files | symbols | cold index | no-op reindex | 1 file changed | map @1000 tok (actual est.) | dead-code (default) | impact top symbol |
|---|---|---|---|---|---|---|---|---|
| Wraith (Rust) | 34 | 627 | 0.117 s | 0.009 s | 0.042 s | 962 (87/265 symbols) | 1 medium | `Severity`: 170 affected, 8 tests |
| classroom-archiver (JS) | 61 | 364 | 0.108 s | 0.008 s | 0.036 s | 955 (77/305) | 3 medium | `resolve`: 26 affected, 1 test |
| plumbgraph (Rust) | 59 | 696 | 0.176 s | 0.010 s | 0.069 s | 958 (87/362) | 1 medium | `id`: 56, 7 tests |
| chi (Go, third-party) | 84 | 497 | 0.132 s | 0.008 s | 0.049 s | 970 (62/332) | 19 medium | `Handler`: 179, 7 tests |
| gson (Java, third-party) | 264 | 4247 | 0.554 s | 0.013 s | 0.384 s | 978 (41/1331) | 28 medium | `JsonElement`: 1288, 99 tests |

**Seeded recall.** In each repo, 20 uniquely named dead functions and 20 live ones (called from live code) were injected in the repo's own language (outside test/example/bench/fixture paths). All 20/20 dead were found at the default threshold and 0/20 live were flagged. This is a synthetic, easy test (unique names, simple call shapes); it shows the pipeline works, not real-world recall.

**Not measured:** precision of the dead-code findings on chi and gson (19 and 28 findings were not hand-reviewed), the Go/Java/C# cases beyond fixtures (C# was not benchmarked on a real repository; no C# import resolution), real-tokenizer map sizes, whether maps improve agent task success, and any run of external providers (semgrep, ast-grep, SCIP indexers) inside `verify`; those paths are covered by fixture/fake-tool tests only. `verify` on plumbgraph itself exits 1 because `check-deps` reports 10 findings, not triaged here.
