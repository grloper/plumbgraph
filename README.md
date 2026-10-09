<p align="center"><img src="assets/logo.svg" width="120" alt="Plumbgraph logo"></p>

<h1 align="center">Plumbgraph</h1>
<p align="center"><b>Know your code. Verify the change.</b><br>
Code intelligence and pre-submit verification for AI coding agents.</p>

<p align="center">
<a href="https://github.com/grloper/plumbgraph/actions/workflows/ci.yml"><img src="https://github.com/grloper/plumbgraph/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<img src="https://img.shields.io/badge/status-pre--alpha%20v0.2-orange" alt="status: pre-alpha">
<img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="license">
</p>

> **Status: pre-alpha (v0.2).** Not released: no crate, no binaries, no tags. Heuristic results; every finding says where it came from and how sure it is. See [what does not work yet](#what-does-not-work-yet).

Plumbgraph (CLI: `plumb`) indexes your repository with tree-sitter into a small SQLite graph and answers the questions an AI coding agent should ask **before** it submits a change:

- Is this code actually used? (`dead-code`)
- Does every import resolve to a declared, *real* package, or did the model hallucinate one? (`check-deps`)
- Did the change quietly weaken the tests to get green? (`weakening`)

It runs locally, executes none of your project's code, and speaks [MCP](https://modelcontextprotocol.io) over stdio so agents can call it directly.

## What works (v0.2)

Everything in this table is covered by tests in this repository (see `scripts/verify.sh`).

| Capability | Languages | Notes |
|---|---|---|
| `plumb index <path>` | Python, JavaScript, TypeScript/TSX, Rust (+ HTML/config text scanned for name mentions) | Symbols, imports, references, calls where name-resolvable. Incremental (content hash). |
| `plumb dead-code <path>` | same | Reachability from entry points/tests (`--lib` also treats exports as public API), confidence 0-1 (`high >= 0.9`, `medium >= 0.7`, `low` hidden unless `--min-confidence`), "used only by tests" kept separate, allow-list file and `plumb:keep` comments, evidence and false-positive risks per finding. |
| `plumb check-deps <path>` | Python, JS/TS, Rust | Imports vs `pyproject.toml`/`requirements.txt`, `package.json`, `Cargo.toml`; registry existence on PyPI / npm / crates.io (`--offline` disables network). Registry errors are never reported as "does not exist". |
| `plumb weakening --base <rev>` | any language with test files | Parses `git diff`: deleted test files/tests, new skip/ignore/only markers, trivial or fewer/looser assertions. |
| `plumb find`, `plumb refs` | same as index | Symbol lookup and incoming references with per-edge confidence and source (`scip` when an index resolved it, else name-based). |
| `plumb diagnostics <path>` | Rust, TypeScript/JavaScript, Python (+ any LSP server) | Normalises real `cargo check`/clippy JSON, `tsc`, `pyright --outputjson`, `ruff --output-format json` output, or LSP `publishDiagnostics`, into findings with `source`, `confidence`, line and column. `--from FORMAT:FILE` only reads saved output; `--run` / `--lsp` execute the project's toolchain and are opt-in. See [docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md). |
| SCIP overlay (`index.scip`, `--scip`, `plumb scip`) | rust-analyzer, scip-typescript, scip-python indexes | Replaces name-based edges with compiler-grade ones where the index resolved a reference, for `refs` and `dead-code`; findings say `scip` or `t0-treesitter`. Falls back per reference. See [docs/SCIP.md](docs/SCIP.md). |
| `plumb mcp` | | MCP server over stdio exposing the tools above plus `diagnostics` and `scip_status`. Hand-rolled minimal JSON-RPC 2.0 (`initialize`, `ping`, `tools/list`, `tools/call`). |

Every finding carries `source` (e.g. `t0-treesitter`, `scip`, `compiler:rustc`, `lint:ruff`, `typecheck:tsc`, `lsp:rust-analyzer`, `manifest+t0-import`, `git-diff`) and `confidence`. Add `--json` for machine-readable output; `--fail-on high|medium|low` sets the exit code for CI.

Example:

```console
$ plumb dead-code ./my-project
MEDIUM 0.77  src/storage/database.js:237  method `ArchiveDatabase.updateMaterialPath` appears unused  [t0-treesitter]
         why:  no reference edges from any code (0 incoming)
         why:  not reachable from entry points (...)
         risk: exported/public: may be used by consumers outside the indexed code
         risk: tier-0 analysis is name-based: it does not see reflection, macros, or code outside the indexed files
```

How well does it do on real code? Only two small sanity runs (name-based and SCIP-assisted) so far, and no measured precision/recall: see [docs/EVALUATION.md](docs/EVALUATION.md).

## Install

Nothing is published yet. Build from source (Rust 1.85+; the pinned toolchain is picked up automatically by `rustup`):

```bash
git clone https://github.com/grloper/plumbgraph
cd plumbgraph
cargo install --path crates/cli --locked      # installs `plumb` and `plumbgraph`
plumb --help
```

## Use with an agent (MCP)

`plumb mcp` serves over stdio; the project root is `--root` (default: current directory) and tool paths are confined to it. Tools: `index_project`, `find_symbol`, `references`, `dead_code`, `check_dependencies`, `detect_test_weakening`, `scip_status`, `diagnostics`. Results include `untrusted: true`: text derived from repository files is data, not instructions. `references`/`dead_code` use a SCIP index when present (`use_scip=false` to disable) and say so in `index.tiers`. `diagnostics` can ingest saved output always; running tools or language servers (`run`, `lsp`) is refused unless **you** start the server with `plumb mcp --allow-exec`.

**Claude Code**

```bash
claude mcp add plumbgraph -- plumb mcp --root /path/to/project
```

**Cursor** (`.cursor/mcp.json`), **Claude Desktop** and most other clients that use the `mcpServers` format:

```json
{
  "mcpServers": {
    "plumbgraph": {
      "command": "plumb",
      "args": ["mcp", "--root", "/path/to/project"]
    }
  }
}
```

Only the stdio protocol basics are implemented and tested against our own client; please report incompatibilities with specific clients.

## What does not work yet

- **Precise only where you supply an index.** Without a SCIP index, analysis is tree-sitter and name based: no type information, no cross-language edges, overloaded or common names produce ambiguous, lower-confidence edges. With one, precision depends on the indexer (calls on untyped receivers in JS/Python are often not resolved and stay name-based). Plumbgraph never runs an indexer. See [docs/SCIP.md](docs/SCIP.md).
- **Diagnostics are passthrough with fixed confidences** (not calibrated), only for cargo/clippy, tsc, pyright, ruff and LSP push diagnostics; no baseline diffing yet ([docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md)).
- Dynamic features (reflection, `getattr`, `eval`, DI containers, item-generating macros, framework magic) are invisible; they lower confidence but are not understood.
- Dead-code recall is **not measured**; precision was only sanity-checked on two small repos, name-based vs SCIP on the same two ([docs/EVALUATION.md](docs/EVALUATION.md)). scip-python was only seen working on a toy and crashed on one real directory.
- Language packs can only choose among the built-in grammars; runtime-loaded grammars are not implemented ([docs/PACKS.md](docs/PACKS.md)).
- Only Python, JS/TS, Rust. Monorepo/workspace resolution is basic.
- `weakening` is heuristic (line-based); it cannot know that a deleted test was redundant.
- MCP: stdio only, tools only (no resources/prompts), protocol version `2024-11-05`.
- No releases, no packaging, no Windows testing.

## Roadmap

Directional, not promises.

1. Harden v0.1: more fixtures per language, real-world evaluation with measured precision/recall.
2. ~~Semantic tier: SCIP ingestion and diagnostics~~ (v0.2, see above). Next: baseline-diffing of diagnostics, SCIP implementation relationships, a managed LSP pool, more indexers/languages.
3. More languages via runtime-loadable packs.
4. Pre-submit "verify the change" bundle: one call that runs dead-code on the diff, dependency check, weakening and reports a single verdict.
5. Releases, prebuilt binaries, crates.io (owner decision).

## How it relates to other tools

Plumbgraph is complementary to, not a replacement for, tools such as [Serena](https://github.com/oraios/serena) (semantic, LSP-backed code navigation and editing for agents) and code-graph MCP servers like CodeGraph: those focus on helping an agent *navigate and edit*. Plumbgraph's focus is the *verification* side (dead code, hallucinated or undeclared dependencies, weakened tests) with an explicit confidence on every finding, and a deliberately small local tree-sitter core. Use them together; if you need precise cross-reference navigation today, an LSP-backed tool will be more accurate than our tier-0 graph. Dedicated linters and dead-code tools (vulture, knip, `cargo udeps`, ...) are more mature per language; Plumbgraph's angle is one agent-facing interface across languages.

## Development

```bash
scripts/verify.sh          # fmt, clippy -D warnings, tests, test-integrity, cargo-deny/audit if installed
```

See [CONTRIBUTING.md](CONTRIBUTING.md), [AGENTS.md](AGENTS.md), [SECURITY.md](SECURITY.md). Licensed under [Apache-2.0](LICENSE).
