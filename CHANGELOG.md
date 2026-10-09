# Changelog

## [1.0.0] - DRAFT (unreleased, awaiting owner approval; not tagged, not published)

### Added
- `plumb map`: ranked (PageRank), token-budgeted repo map; changed-file aware (`--base`).
- `plumb impact <symbol|--diff>`: transitive callers and tests to run, with per-path confidence.
- `plumb verify`: one verdict over dead-code, check-deps, weakening, diagnostics and rule engines; baseline file (`plumb-baseline.json`) so only new findings fail; `--run` is opt-in.
- Provider orchestrator (`plumb doctor`, `plumb enrich`): detects installed SCIP indexers, linters, rule engines (semgrep, ast-grep); runs them only with explicit opt-in.
- `plumb init`: writes AGENTS.md block and MCP configs (idempotent, non-clobbering).
- Go, Java, C# tree-sitter packs (tier-0, name-based).
- MCP: 13 tools.
- Incremental indexing: mtime skip, resolve stamp, cached query compilation.
- docs/LANDSCAPE.md, benchmark script `scripts/bench.py`, results in `docs/bench/`.

### Fixed
- Rust `mod` resolution for Cargo targets under `tests/`, `benches/`, `examples/`, `src/bin/`.
- Argument injection guard for git revisions.

### Known gaps
See README "What does not work yet" and docs/EVALUATION.md.
