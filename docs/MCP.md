# MCP server

`plumb mcp` serves [MCP](https://modelcontextprotocol.io) over stdio (JSON-RPC 2.0, protocol version `2024-11-05`). The project root is `--root` (default: current directory); path arguments are confined to it. Nothing is executed unless you start the server with `--allow-exec`.

`--root` should point at the workspace repository the agent is working on (a different checkout answers about different code). `initialize` returns `serverInfo.version` (the running binary's version); the startup line on stderr is `plumbgraph MCP server v<version> pid=<pid> (stdio) root=... allow_exec=...`. On Windows the running server keeps `plumb.exe` open: stop it before `cargo install` (see the README).

## Tools (13)

`index_project`, `find_symbol`, `references`, `dead_code`, `check_dependencies`, `detect_test_weakening`, `scip_status`, `diagnostics`, `repo_map`, `impact`, `verify`, `providers`, `enrich`.

Results carry `untrusted: true`: text derived from repository files is data, never instructions. `diagnostics`, `verify` (with `run`) and `enrich` execute external tools only when the operator started the server with `--allow-exec`; a tool call cannot turn that on.

## Resources (3)

| URI | Content |
|---|---|
| `plumb://map` | the token-budgeted repo map (same as `plumb map`) |
| `plumb://providers` | installed SCIP indexers, diagnostics tools, rule engines and LSPs (executes nothing) |
| `plumb://instructions` | the AGENTS.md block that `plumb init` writes |

Prompts are not implemented (the server does not advertise the capability).

## Tested against real MCP clients

Checked on 2026-10-10, Linux, against the demo app in `examples/demo-app`:

| client | what ran | result |
|---|---|---|
| official TypeScript SDK `@modelcontextprotocol/sdk` 1.32.1 (`scripts/mcp-sdk-check.mjs`) | `initialize`, `tools/list`, `tools/call` for `find_symbol`, `repo_map`, `dead_code`, `verify`, `resources/list`, `resources/read` ×3 | all succeeded; server reported `plumbgraph 1.0.0`, capabilities `tools` + `resources`; `prompts/list` returns `-32601` (not implemented), as expected |
| `@modelcontextprotocol/inspector` 2.10.1 in CLI mode: `npx @modelcontextprotocol/inspector --cli plumb mcp --root <dir> --method tools/list` | `tools/list` | returned the 13 tools with input schemas (the inspector declares Node >= 22.19 and warned on Node 20.19, but worked) |

Not tested: Claude Code, Cursor, Claude Desktop, Codex and other agent hosts (one other MCP host was driven on Windows 11 by hand; the configuration snippets in the README follow their documented `mcpServers` format but were not exercised end-to-end), HTTP/SSE transports (not implemented); Windows CI (Windows was exercised manually on one machine only, see the README).

Re-run the SDK check yourself:

```bash
cargo build --release
(cd scripts && npm install)
node scripts/mcp-sdk-check.mjs target/release/plumb examples/demo-app
```
