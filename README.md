<p align="center"><img src="assets/logo.svg" width="112" alt="Plumbgraph logo: a plumb line through a small code graph"></p>

<h1 align="center">Plumbgraph</h1>

<p align="center"><b>Know your code. Verify the change.</b><br>
Local code intelligence and a pre-submit gate for AI coding agents, over CLI and MCP.</p>

<p align="center">
<a href="https://github.com/grloper/plumbgraph/actions/workflows/ci.yml"><img src="https://github.com/grloper/plumbgraph/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License: Apache-2.0"></a>
<img src="https://img.shields.io/badge/version-1.0.0%20(unreleased)-informational" alt="Version 1.0.0, unreleased">
<img src="https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust" alt="Rust 1.85+">
<img src="https://img.shields.io/badge/MCP-stdio-5B5BD6" alt="MCP over stdio">
</p>

<p align="center"><img src="docs/demo/verify.svg" alt="Terminal recording of `plumb verify --run --online` on the demo app: a hallucinated import, an unused function, an eval() and a print() are reported; verdict FAIL" width="860"></p>
<p align="center"><sub>Real output of <code>plumb verify</code> on <a href="examples/demo-app">examples/demo-app</a> (captured by <code>scripts/render-demo.py</code>; raw text in <a href="docs/demo/verify.txt">docs/demo/verify.txt</a>).</sub></p>

## Why

AI agents write code fast and check it badly. Plumbgraph answers the questions an agent should ask **before** it says "done", and says how sure it is:

- **Is this code used?** `dead-code`, with reachability evidence and false-positive risks.
- **Is this dependency real?** `check-deps` compares imports with your manifests and, optionally, PyPI / npm / crates.io, so a hallucinated package is caught.
- **Did the tests get weaker?** `weakening` reads `git diff` for deleted tests, new skips and loosened assertions.
- **What breaks if I change this?** `impact` lists transitive callers and the tests to run; `map` gives a token-budgeted overview.
- **One verdict.** `verify` merges all of that with diagnostics, semgrep and ast-grep, diffs against a baseline, and fails only on **new** findings.

It indexes with tree-sitter into a local SQLite file, executes none of your code by default, and every finding carries a `source` and a `confidence`.

## Quickstart

Nothing is published yet (no crate, no binaries). Build from source with Rust 1.85+:

```bash
git clone https://github.com/grloper/plumbgraph && cd plumbgraph
cargo install --path crates/cli --locked      # installs `plumb` (and `plumbgraph`)

plumb map .                       # ranked, token-budgeted overview
plumb dead-code .                 # add --lib for libraries (exports = public API)
plumb check-deps . --online       # sends package names only to the registries
plumb verify .                    # one pass/fail gate (exit 1 on new findings)
plumb verify . --run --online     # also run diagnostics, semgrep, ast-grep (trusted code only)
plumb init                        # write AGENTS.md block + MCP config for your agent
```

`--json` on any command gives machine-readable output; `--fail-on high|medium|low` sets the exit code.

### Use with an agent (MCP)

```bash
claude mcp add plumbgraph -- plumb mcp --root /path/to/project
```

Or in `.cursor/mcp.json`, Claude Desktop and other `mcpServers` clients:

```json
{ "mcpServers": { "plumbgraph": { "command": "plumb", "args": ["mcp", "--root", "/path/to/project"] } } }
```

13 tools and 3 resources; running external tools is refused unless **you** start the server with `--allow-exec`. Details and the clients it was actually tested with: [docs/MCP.md](docs/MCP.md).

## See it work

| | |
|---|---|
| **Map and impact** | <img src="docs/demo/map-impact.svg" alt="plumb map and plumb impact on the demo app" width="520"> |
| **SCIP makes it precise** | <img src="docs/demo/scip.svg" alt="plumb enrich runs scip-python; dead-code then reports the finding from SCIP" width="520"> |

All images are renderings of real runs; regenerate them with `scripts/render-demo.py`.

## How it works

```mermaid
flowchart LR
  SRC[Source files] -->|tree-sitter packs| DB[(SQLite graph<br/>edges carry source + confidence)]
  SCIP[SCIP indexes<br/>optional] -->|overlay| DB
  DB --> ENG[dead-code · map · impact]
  MAN[Manifests + registries] --> DEPS[check-deps]
  GIT[git diff] --> WK[weakening]
  TOOLS[clippy · tsc · ruff · pyright · LSP<br/>semgrep · ast-grep<br/>opt-in subprocesses] --> V
  ENG --> V[verify<br/>baseline diff]
  DEPS --> V
  WK --> V
  V --> OUT[plumb CLI / MCP server]
```

More in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## What works

| Capability | Languages | Notes |
|---|---|---|
| `index`, `find`, `refs` | Python, JavaScript, TypeScript/TSX, Rust, Go, Java, C# | Tier-0 (tree-sitter, name-based). Import resolution varies by language; C# imports are not resolved to files. |
| `dead-code` | same | Reachability from entry points and tests; confidence 0-1; `--lib`, allow-list (`.plumbgraph/allow.toml`), `plumb:keep` comments; evidence and false-positive risks per finding. |
| `check-deps` | Python, JS/TS, Rust | Imports vs manifests; registry existence checks are opt-in (`--online`), and registry errors are never reported as "does not exist". |
| `weakening` | any language with test files | Heuristic, line-based `git diff` analysis. |
| `map`, `impact` | all of the above | PageRank-ranked map under a hard token budget; reverse reachability with tests to run. |
| `verify`, `doctor`, `enrich`, `init` | | Baseline-diffing gate; provider detection; opt-in SCIP indexers; agent config writer. |
| SCIP overlay | rust-analyzer, scip-typescript, scip-python indexes | Compiler-grade edges where the index resolved a reference, per-reference fallback ([docs/SCIP.md](docs/SCIP.md)). Exercised on toy projects with scip-python and scip-typescript. |
| Diagnostics | Rust, TS/JS, Python, any LSP | Normalised cargo/clippy, tsc, pyright, ruff, LSP `publishDiagnostics` ([docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md)). |
| Rule engines | semgrep, ast-grep | Run as subprocesses with `--run`; semgrep needs `.semgrep.yml` or `--semgrep-config`, ast-grep needs `sgconfig.yml`. |

## How it compares

Facts below come from each project's own README/docs on 2026-10-10 (links in [docs/LANDSCAPE.md](docs/LANDSCAPE.md)); we did not benchmark any of them, and "not documented" means we did not find it, not that it is absent. Star counts are not used.

| | **Plumbgraph** | [Serena](https://github.com/oraios/serena) | [CodeGraph](https://github.com/colbymchenry/codegraph) | [Aider repo map](https://aider.chat/docs/repomap.html) | [Sourcegraph](https://sourcegraph.com/docs/code-navigation) |
|---|---|---|---|---|---|
| Main job | Verify a change (dead code, fake deps, weakened tests) plus navigation | Semantic retrieval and editing for agents | Pre-indexed code knowledge graph for agents | Give the LLM a ranked map of the repo | Code search and navigation platform |
| How it analyses | tree-sitter; optional SCIP and LSP-diagnostics overlays | Language servers (LSP) | Local code graph (see its docs for details) | tree-sitter definitions/references, graph ranking | Search-based, plus precise navigation from SCIP indexes |
| Interface | CLI, MCP (stdio) | MCP | MCP, CLI | Inside aider | Web UI, API; MCP server on the enterprise plan |
| Runs locally, no service | Yes | Yes | Yes ("100 % local") | Yes | Self-hosted or single-tenant cloud |
| Dead-code / hallucinated-dependency / test-weakening checks | Yes | Not documented | Not documented | No (not its purpose) | Not documented |
| Confidence and source on every result | Yes | Not documented | Not documented | n/a | n/a |
| Precise cross-references | Only with a SCIP index; name-based otherwise | Yes, via LSP | Not verified | Not type-resolved (tree-sitter) | Yes, with SCIP indexes |
| License | Apache-2.0 | GPL-3.0-or-later (app), per its README | MIT | Apache-2.0 | Commercial (enterprise from $16K/year, pricing page) |

Use them together: if you need precise navigation and editing today, an LSP-backed tool such as Serena will beat our tier-0 graph; Plumbgraph's niche is the verification side and honest provenance.

## What does not work yet

- Without a SCIP index, analysis is **name-based**: no types, no cross-language edges; common names give ambiguous, lower-confidence edges.
- **Dead-code precision is low on libraries in the default (app) mode** and recall is unmeasured on real code. Hand review of 76 findings on chi and gson found 1 real unused function; four systematic false-positive classes were fixed ([docs/EVALUATION.md](docs/EVALUATION.md)). Use `--lib` for libraries.
- Reflection, `eval`, DI containers, macros and framework magic are invisible (they lower confidence but are not understood).
- Go, Java, C# are tier-0 only; C# was not benchmarked on a real repository.
- SCIP: rust-analyzer's SCIP output, scip-go, scip-java and scip-dotnet were not run; scip-python and scip-typescript only on toys.
- MCP: stdio only, tools and resources only (no prompts). Tested with the official TypeScript SDK and the MCP inspector CLI; agent hosts were not tested.
- `weakening` is line-based and heuristic. No Windows testing. No releases or packages yet.

## Documentation

[Docs index](docs/README.md) · [Architecture](docs/ARCHITECTURE.md) · [MCP](docs/MCP.md) · [SCIP](docs/SCIP.md) · [Diagnostics](docs/DIAGNOSTICS.md) · [Language packs](docs/PACKS.md) · [Evaluation](docs/EVALUATION.md) · [Landscape](docs/LANDSCAPE.md) · [Changelog](CHANGELOG.md)

## Development

```bash
scripts/verify.sh            # fmt, clippy -D warnings, tests, test-integrity, cargo-deny/audit if installed
plumb verify .               # plumbgraph checks itself (allow-list with reasons in .plumbgraph/allow.toml)
```

See [CONTRIBUTING.md](CONTRIBUTING.md), [AGENTS.md](AGENTS.md) and [SECURITY.md](SECURITY.md). Licensed under [Apache-2.0](LICENSE).
