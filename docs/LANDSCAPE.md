# Landscape: what exists, what Plumbgraph learns from it, what it deliberately does not do

Facts below (license, stars, last push, archived flag) were read from the GitHub REST API
(`api.github.com/repos/<owner>/<repo>`) on 2026-10-10; star counts are snapshots. READMEs/docs
were read via the linked URLs. "Not verified" means we did not check it ourselves. Nothing here
is a benchmark of those tools; we did not run most of them.

| Tool | What it does (per its docs) | License / stars / last push | What Plumbgraph takes from it |
|---|---|---|---|
| Aider repo map — https://aider.chat/docs/repomap.html , source https://github.com/Aider-AI/aider | tree-sitter definitions + references, file graph, graph ranking, packed into a token budget (`--map-tokens`, default 1k; "not a hard limit") | Apache-2.0 / 49 435 / 2026-05-22 | **Map design**: ranked graph + budget fit. We rank *symbols* with PageRank over our resolved edges, personalise on changed files, and treat the budget as a hard cap (`plumb map --tokens`). |
| Serena — https://github.com/oraios/serena | MCP toolkit: symbol-level retrieval/editing via LSP (and a JetBrains plugin) | NOASSERTION in API; its LICENSE has per-component terms (SolidLSP MIT, app GPL-3.0-or-later) / 30 132 / 2026-10-09 | **Use, don't embed**: its GPL app code is not copied. Plumbgraph orchestrates LSP servers itself (diagnostics) and documents Serena as a complementary *editing* tool. |
| CodeGraph — https://github.com/colbymchenry/codegraph | local tree-sitter graph + MCP, auto-configures agents | MIT / 73 605 / 2026-10-07 | **UX**: `plumb init` writes agent config snippets. Its benchmark claims are self-reported; we did not verify them. |
| code-review-graph — https://github.com/tirth8205/code-review-graph | tree-sitter → SQLite graph, blast radius for review, MCP | MIT / 32 006 / 2026-10-06 | **Concept**: blast radius → `plumb impact`. We add confidence+provenance per edge and tests-to-run. |
| SCIP — https://github.com/scip-code/scip | protobuf index format: definitions, references | Apache-2.0 / 826 / 2026-10-07 | **Tier-1 ingest** (already in v0.2); v1 orchestrator can *run* an installed indexer (opt-in). |
| SCIP indexers — https://github.com/sourcegraph/scip-typescript , https://github.com/scip-code/scip-java , https://github.com/scip-code/scip-go , https://github.com/sourcegraph/scip-dotnet | compiler-grade indexes per language | Apache-2.0 each / 37–135 | **Providers**: detected on PATH, run only with `--run-providers`. Rust via `rust-analyzer scip`. |
| Semgrep — https://github.com/semgrep/semgrep | pattern/taint static analysis | LGPL-2.1 / 16 945 / 2026-10-09 | **Provider via subprocess only** (LGPL: never linked). Its JSON findings are normalised into the same finding model. |
| ast-grep — https://github.com/ast-grep/ast-grep | tree-sitter structural search/lint, YAML rules | MIT / 16 173 / 2026-10-09 | **Provider** (`sg scan --json`), subprocess. |
| tree-sitter — https://github.com/tree-sitter/tree-sitter | incremental parser runtime + grammars | MIT / 27 144 / 2026-10-09 | Tier-0 extractor (already). v1 adds Go, Java, C# packs. |
| stack-graphs — https://github.com/github/stack-graphs | zero-build name resolution on tree-sitter | Apache-2.0 / 874 / **archived** (2025-09-09) | Not used (unmaintained); our heuristic resolver labels confidence instead. |
| knip — https://github.com/webpro-nl/knip | unused files/exports/deps for JS/TS | ISC / 12 429 / 2026-10-09 | Recommended to run for JS/TS beside `plumb`; not wrapped in v1 (listed gap). |
| vulture — https://github.com/jendrikseipp/vulture | Python dead code with 60–100 % confidence | MIT / 4 840 / 2026-09-25 | Confidence-score idiom. |
| mcp-language-server — https://github.com/isaacphi/mcp-language-server | one LSP exposed over MCP (definition, references, diagnostics, rename) | BSD-3-Clause / 1 607 / 2026-03-01 | Confirms diagnostics-over-MCP is wanted; Plumbgraph merges LSP diagnostics with other sources instead of one-server-per-process. |
| LSP spec — https://microsoft.github.io/language-server-protocol/ | editor↔server protocol | spec | `publishDiagnostics` client (v0.2). |
| MCP spec — https://modelcontextprotocol.io/specification | tools, resources, prompts over JSON-RPC | spec | v1 server implements tools **and resources** (`plumb://map`, …). |
| Sourcegraph — https://sourcegraph.com/docs/code-navigation , https://sourcegraph.com/pricing | code search and code navigation platform; "precise" navigation from SCIP indexes, search-based navigation otherwise; MCP server on the enterprise plan | commercial (pricing page read 2026-10-10: enterprise "starting at $16K" minimum annual contract) | **Format and vocabulary**: SCIP is Sourcegraph's index format. Plumbgraph is a local CLI/MCP tool, not a hosted platform. |
| Go tooling — https://github.com/golang/tools | `deadcode`, gopls | BSD-3-Clause / 8 006 | Not wrapped (gap); Go gets tier-0 only. |

## Where Plumbgraph is different (and where it is not)

* Not different: parsing (tree-sitter), index formats (SCIP), ranked repo maps (Aider), blast radius
  (code-review-graph), linters/LSPs (everyone). Plumbgraph does not replace any of them.
* The niche: **one graph where every fact has provenance and confidence**, fed by whatever is
  installed, plus a **verification gate** (`plumb verify`): dead code, hallucinated deps, test
  weakening and diagnostics diffed against a baseline so only *new* problems fail.
* Honest limits: tier-0 resolution is name-based; precision beyond that depends on the external
  providers actually being installed. We have not benchmarked against the tools above.
