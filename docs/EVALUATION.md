# Evaluation (sanity runs, not benchmarks)

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
- No semantic (type-aware / LSP / SCIP) tier yet.
