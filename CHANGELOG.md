# Changelog

## [1.0.2] - 2026-10-10

- Docs only: README now matches the released version, the tested Windows scope, supported weakening languages and known limits.

## [1.0.1] - 2026-10-10

Fixes for problems found by using v1.0.0 on real projects (Windows, Unity). Not released; the crate version is still 1.0.0.

### Fixed
- Windows: `plumb doctor` (and `enrich`, rule-engine detection in `verify --run`) reported every provider as missing because lookup used the bare name (`cargo`), never `cargo.exe` / `tsc.cmd`. Bare names are now tried with the `PATHEXT` extensions that `std::process::Command` can start (`.com`, `.exe`, `.bat`, `.cmd`); Unix lookup is unchanged.
- `plumb init` printed a Codex `config.toml` snippet with a `\\?\C:\...` path inside a TOML basic string, which does not parse. The verbatim prefix is stripped (`\\?\UNC\` becomes `\\`) and the path is written as a TOML literal string, or an escaped basic string when it contains `'` or control characters. Report paths no longer show `\\?\` either.
- `plumb map --budget N` was rejected; `--budget` is now a visible alias of `--tokens`.
- `plumb init` and `.gitignore`: `--dry-run` shows the lines it would add; it writes `.plumbgraph/*` + `!.plumbgraph/allow.toml` (the previous `.plumbgraph/` made the committed allow-list impossible to commit); any existing rule mentioning `.plumbgraph` (including `!.plumbgraph/`) is respected; it works in a subdirectory of a repository; CRLF files keep CRLF; a `.gitignore` that is not UTF-8 (for example UTF-16 from PowerShell 5) is no longer read as empty and overwritten.

### Added
- C# / Unity dead-code entry points: MonoBehaviour/ScriptableObject/NetworkBehaviour/StateMachineBehaviour/UIBehaviour messages (`Awake`, `Start`, `Update`, `OnTriggerEnter2D`, ...), Unity attributes (`[RuntimeInitializeOnLoadMethod]`, `[MenuItem]`, `[InitializeOnLoad]`, ...), and Unity callback interfaces (`ISerializationCallbackReceiver`, `IHasCustomMenu`, UI EventSystem handlers). Public methods of Unity classes stay findings with a 0.25 penalty and a UnityEvent / Animation Event fp_risk. Rules and limits: `docs/PACKS.md`. On Unity's open-project-1 scripts (commit 608eac9) dead-code findings went from 371 to 166 (medium 97 to 28); on Newtonsoft.Json (52fa3ae) they are identical before and after (204).
- `plumb init --no-gitignore`.
- README: Windows install and PATH setup, and exactly what was tested on Windows.

### Changed
- Index schema 3 / extractor 8 (new `framework_risk` column): existing indexes are rebuilt once.

## [1.0.0] - 2026-10-10

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
