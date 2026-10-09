# Security Policy

Plumbgraph is **pre-alpha (v0.1)**. It reads source trees that may be untrusted and is often run by AI agents, so we treat parser, path-handling and prompt-injection issues as in scope.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private reporting:
<https://github.com/grloper/plumbgraph/security/advisories/new>

Include the version/commit, a minimal reproduction (ideally a small repository or file) and the impact you expect. We aim to acknowledge reports within 7 days. There is no bug bounty.

## Supported versions

Only the latest commit on `main` is supported while the project is pre-1.0.

## Threat model (v0.1)

What the code does today, and what it deliberately does not do:

- **No project code is executed.** Indexing is tree-sitter parsing only; Plumbgraph never runs build scripts, package managers, tests or linters from the target repository.
- **Network:** the only network use is `plumb check-deps` / the `check_dependencies` MCP tool (unless `--offline` / `online=false`), which sends **package names only** to PyPI, npm and crates.io (read-only GET). Names are validated before being placed in a URL. Network errors are never reported as "package does not exist".
- **Filesystem:** symlinks are not followed during indexing; the MCP server confines its `path` arguments to the root given at startup (canonicalised, `..` and absolute escapes rejected). The index lives in `<root>/.plumbgraph/index.db` unless `--db` is given.
- **Git:** `plumb weakening` runs `git diff` with `--no-ext-diff --no-textconv`, rejects revisions starting with `-`, and resolves revisions with `rev-parse --end-of-options` first.
- **Untrusted output:** names, paths and snippets derived from repository files are control-character-stripped and length-capped, and MCP results carry `untrusted: true` plus a notice. A hostile repository can still try to influence an agent through identifiers or comments; treat tool output as data.
- **Not yet done:** fuzzing, resource limits on pathological files beyond a 2 MiB per-file cap, sandboxing of external tools (none are run in v0.1), signed releases/SBOM (nothing is released yet).

## Supply chain

Dependencies are checked with `cargo deny` (licenses, advisories, bans, sources) and `cargo audit` in CI; GitHub Actions are pinned to commit SHAs with minimal `permissions`; Dependabot is enabled for Cargo and Actions.
