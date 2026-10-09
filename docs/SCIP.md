# SCIP: precise references on top of the name-based graph (v0.2)

[SCIP](https://github.com/scip-code/scip) is a protobuf index format with compiler-grade definitions and references. Plumbgraph **reads** SCIP files; it never runs an indexer for you.

## Producing an index

| Language | Indexer | Command (run it yourself, in the project root) |
|---|---|---|
| Rust | rust-analyzer | `rust-analyzer scip . --output index.scip` |
| TypeScript / JavaScript | [scip-typescript](https://github.com/sourcegraph/scip-typescript) | `scip-typescript index --infer-tsconfig` (JS projects without a `tsconfig.json`) or `scip-typescript index` |
| Python | [scip-python](https://github.com/sourcegraph/scip-python) | `scip-python index . --project-name NAME` (version 0.6.6 worked on a 14-line toy and crashed on Wraith's `scripts/`, see [EVALUATION.md](EVALUATION.md)) |

Indexers execute or analyse project code and toolchains (rust-analyzer loads build scripts and proc-macros), so run them on code you trust.

## Using it

`plumb dead-code`, `plumb refs` and the MCP tools `dead_code` / `references` pick up `<root>/index.scip` or `<root>/.plumbgraph/index.scip` automatically. Use `--scip FILE` (repeatable) for other locations, `--no-scip` to force the name-based analysis. An explicitly given or auto-detected index that cannot be parsed is an **error**, never a silent fallback. `plumb scip <path>` (MCP: `scip_status`) reports how much of the project an index covers.

## Exactly what the overlay does

For each indexed file that the SCIP index covers and that is **not newer than the index file** (mtime check; newer files are counted as `stale_files` and stay name-based):

1. Definitions in the index are mapped to graph symbols by file, line and name. A reference binds to the definition in its own document; if the same SCIP symbol string is defined in several documents (rust-analyzer does this for same-named items in different binary targets) and the reference's own document does not define it, it stays unresolved.
2. A name-based edge `(file, line, name)` is replaced by the SCIP edge (`source = "scip"`, confidence 0.97) **only if the index resolved an occurrence of that name on that line** (to a project symbol, or to a symbol defined outside the project). Occurrences the index did not resolve keep their name-based edge. This matters: indexers cannot resolve calls on untyped receivers (plain JS, unannotated Python), and our first attempt that dropped those edges wrongly reported 18 live classroom-archiver methods as dead (see [EVALUATION.md](EVALUATION.md)).
3. Files the index does not know keep all name-based edges.
4. Dead-code findings for symbols the index defines get `source: "scip"`; the "name occurs nowhere else" bonus and the "imported elsewhere" penalty are not applied to them (SCIP distinguishes same-named symbols), but string-literal, dynamic-access, decorator, subclass and exported penalties still are. Everything else stays `t0-treesitter`.

## Known gaps

- Only `Definition` and plain reference occurrences are used. SCIP `relationships` (trait/interface implementations) are ignored, so implementation methods are still handled by the tier-0 entry-point rules.
- Local symbols (`local N`) are not mapped; only top-level and member symbols with a global SCIP symbol.
- Index paths must equal Plumbgraph's project-relative paths (index produced at the project root). Indexes of sub-projects or monorepos with a different root show up as `documents_unmatched`.
- Staleness is an mtime heuristic.
- scip-python: automated tests use a hand-built index in the same format; the real indexer was only tried on a 14-line toy (worked) and on Wraith's `scripts/` (crashed with a stack trace, no usable index). No Python measurement exists.
- No SCIP support for languages Plumbgraph has no tree-sitter pack for.
