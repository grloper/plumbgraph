# Architecture

```mermaid
flowchart LR
  subgraph Sources
    SRC[Source files<br/>py js ts tsx rs go java cs]
    MAN[Manifests<br/>Cargo.toml package.json pyproject ...]
    GIT[git diff]
  end
  subgraph Providers["Optional providers (opt-in, run as subprocesses)"]
    SCIP[SCIP indexers<br/>scip-typescript scip-python rust-analyzer]
    DIAG[Diagnostics<br/>cargo clippy tsc ruff pyright LSP]
    RULES[Rule engines<br/>semgrep ast-grep]
    REG[Package registries<br/>PyPI npm crates.io]
  end
  SRC -->|tree-sitter packs| X[Tier-0 extractor]
  X --> DB[(SQLite graph<br/>symbols + edges<br/>source + confidence)]
  SCIP -->|.scip overlay| DB
  DB --> ENG
  MAN --> ENG
  GIT --> ENG
  DIAG --> ENG
  RULES --> ENG
  REG --> ENG
  subgraph ENG[Engines]
    DC[dead-code]
    CD[check-deps]
    WK[weakening]
    MP[map / impact]
    VF[verify<br/>baseline diff]
  end
  ENG --> CLI[plumb CLI]
  ENG --> MCP[plumb mcp<br/>tools + resources]
```

* **Tier 0** (always on): tree-sitter extraction into a local SQLite file (`.plumbgraph/index.db`), incremental by content hash and mtime. Edges are name-resolved and carry a confidence.
* **Tier 1** (if you have an index): a SCIP overlay replaces name-based edges with compiler-grade ones per occurrence and falls back per reference ([SCIP.md](SCIP.md)).
* **Providers** run only when you ask (`--run`, `plumb enrich`, MCP `--allow-exec`); their output is normalised into the same finding model (`source`, `confidence`, `level`, `evidence`, `fp_risks`).
* **verify** merges every engine, diffs against `plumb-baseline.json` and fails only on *new* findings at or above `--fail-on`.

Crates: `plumbgraph-langs` (pack loader + grammars), `plumbgraph-core` (extraction, store, engines, `ops` shared by CLI and MCP), `plumbgraph-mcp` (hand-rolled stdio server), `plumbgraph-cli` (the `plumb` binary).
