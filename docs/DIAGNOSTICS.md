# Diagnostics (v0.2)

`plumb diagnostics` (MCP: `diagnostics`) normalises real tool output into findings: `category = "diagnostic"`, `rule` = the tool's code, `source` = origin, `column` when known, `confidence`.

| Input | How to get it | `--from` format | `source` |
|---|---|---|---|
| rustc / cargo check | `cargo check --message-format=json` | `cargo-json` | `compiler:rustc` |
| clippy | `cargo clippy --message-format=json` | `cargo-json` | `lint:clippy` (code starts with `clippy::`) |
| tsc | `tsc --noEmit --pretty false` (plain text; tsc has no JSON output) | `tsc` | `typecheck:tsc` |
| pyright | `pyright --outputjson` | `pyright-json` | `typecheck:pyright` |
| ruff | `ruff check --output-format json` | `ruff-json` | `lint:ruff` |
| any language server | `--lsp "rust-analyzer"` (stdio) | | `lsp:<server>` |

```console
$ cargo clippy --message-format=json > clippy.jsonl
$ plumb diagnostics . --from cargo-json:clippy.jsonl
$ plumb diagnostics . --run                       # run applicable tools (executes the toolchain)
$ plumb diagnostics . --lsp "rust-analyzer" --lsp-file src/lib.rs --timeout-secs 120
```

## Execution model

- `--from` only reads files. `executed` is `false` in the report.
- `--run` starts `cargo check` + `clippy` (if `Cargo.toml`), `tsc` (if `tsconfig.json`, preferring `node_modules/.bin/tsc`), `ruff` + `pyright` (Python project files). **`cargo check` runs build scripts and proc-macros; `node_modules/.bin/tsc` is repository-supplied code.** Missing tools are reported as `skipped`, never as failures; a tool that cannot be parsed or times out is `failed`/`timeout`.
- `--lsp CMD` starts the language server, sends `initialize`/`didOpen` for the chosen files (default: up to 100 source files), collects `textDocument/publishDiagnostics`, waits until every file was reported on, the server has been quiet for 1.2 s and any work-done-progress / `experimental/serverStatus` loading phase has ended (or the timeout), then shuts it down. Files must be inside the root.
- MCP: `inputs` is always allowed (paths confined to the root). `run` and `lsp` are refused unless the **operator** started the server with `plumb mcp --allow-exec`; a repository or a prompt cannot turn it on.
- Diagnostics pointing outside the project root are dropped and counted (`dropped_outside_root`). Messages are control-stripped, flattened and capped at 200 characters.

## Confidence

Per-source constants, **not calibrated probabilities** (no measurement backs them): compiler/type-checker error 0.98 (warning 0.93), LSP error 0.95 (warning 0.90), lint 0.88 (warning 0.83), info/hint 0.75. They say "this is what the tool reported"; environment problems (missing dependency, wrong interpreter) can still make a diagnostic spurious, which is listed as a false-positive risk on every finding.

## Known gaps

- No baseline diffing ("only new diagnostics since `<ref>`") yet.
- Only cargo/tsc/ruff/pyright parsers; eslint, mypy, go vet etc. need new parsers.
- LSP: push diagnostics only (no pull diagnostics), no workspace-wide open, one server per `--lsp`. Servers that publish after a long background `cargo check` need a large `--timeout-secs`.
- tsc text parsing ignores continuation lines of multi-line messages.
