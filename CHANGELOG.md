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

- `[[deps_allow]]` allow-list for `check-deps` (path glob, optional package glob, mandatory reason).
- `examples/demo-app` and `scripts/render-demo.py` / `scripts/render-social.py`: README images generated from real runs.
- docs: ARCHITECTURE.md, MCP.md (tested against the official TypeScript SDK and the inspector CLI), docs index, hand-review of chi/gson findings in EVALUATION.md.
- MCP SDK smoke test: `scripts/mcp-sdk-check.mjs`.

### Fixed
- Dead code: Rust `{NAME}` inline format captures count as references; Java `main`, C# `Main`, Java serialization hooks and annotation-driven framework methods are entry points. App mode now suggests `--lib` when exported symbols are reported.
- `plumb verify --fail-on none` prints `REPORT-ONLY` instead of `FAIL`.
- Extractor version bumped: existing indexes are rebuilt once.
- Rust `mod` resolution for Cargo targets under `tests/`, `benches/`, `examples/`, `src/bin/`.
- Argument injection guard for git revisions.

### Known gaps
See README "What does not work yet" and docs/EVALUATION.md.
