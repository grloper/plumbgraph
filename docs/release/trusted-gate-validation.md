# Trusted gate validation (2026-10-11)

Scope: [issue #29](https://github.com/grloper/plumbgraph/issues/29), based on main `5673761ea8227997d68e5f13495c2b3314797c47`. This is local Linux evidence for a proposed release improvement, not a published release or a claim of universal correctness.

Environment: Linux x86_64, Rust/Cargo 1.85.0, Node 24.19.0, official MCP TypeScript SDK from `scripts/package-lock.json`. Rust tools and npm cache were installed in a separate workspace directory; no GitHub Actions, paid CI, persistent runner, billing changes, native Windows/macOS execution, or editor-host setup was used.

## Checks

The final acceptance commands and security-check outcomes are recorded below. Test results count actual executed tests; the integrity baseline counts static test annotations and can differ due to conditional compilation.

- `scripts/verify.sh`: formatting, workspace/all-target clippy with warnings denied, locked workspace tests, test integrity, and offline dogfood gate.
- `cargo test --workspace --release --locked --offline`: optimized build and the same workspace tests.
- `cargo test --locked --offline -p plumbgraph-core --test provider_outcomes --test gate_regression --test diagnostics`: focused failure-injection/ingestion tests.
- `npm ci --ignore-scripts --no-audit --no-fund` with a workspace cache, then `node scripts/mcp-sdk-check.mjs <binary> <root>`: official SDK negotiation, tools/resources, and expected unsupported prompts response.
- Additional official SDK assertions require schema 2, pass/no-exec for the repository gate, malformed request rejection, denied execution/baseline writes without operator authority, and readable resources.
- `git diff --check`: whitespace validation.

The offline gate corpus contains 23 tests: clean changes/refactors; undeclared dependencies without registry claims; weakening/deletion; library/Unity limitations; independent thresholds; baseline edits; invalid bases; malformed saved diagnostics/SCIP; baseline and cache/policy symlink confinement; conservative unobserved entries; and baseline-update failure preservation. Provider tests and unit tests additionally exercise nonzero/signaled/timeout outcomes, oversized/invalid output and inherited pipes. Existing golden expectations and tests were retained; legacy strict-dead-code tests explicitly request the stricter policy.

## Recorded outcomes

- Debug aggregate: **257 executed tests passed**, 0 failed, 0 ignored; integrity baseline increased from 209 to 262 annotations. Formatting, clippy and offline dogfood passed.
- Optimized workspace: **257 executed tests passed**, 0 failed, 0 ignored.
- Focused suites: gate corpus 23, provider outcomes 11, diagnostics 16; all passed.
- Official MCP SDK smoke and assertions: passed, including five malformed/unauthorized verify calls rejected.
- `cargo deny --locked check`, version **0.20.2**: exit 0; advisories, bans, licenses and sources OK. Existing warnings: duplicate versions of getrandom, hashbrown, syn, thiserror, thiserror-impl and windows-sys; unused BSD-2-Clause license allowance. No allow/ignore policy was relaxed.
- `cargo audit`, version **0.22.2**: exit 0; scanned 168 lockfile dependencies against 1,296 advisories with no reported vulnerability.
- Advisory snapshot for both tools: RustSec `7eebec69c352c7191b1f13eb95dd510eeca5d1de`, committed 2026-10-09, fetched 2026-10-11.

Security tools were built from their official published crates with a separate official Rust **1.99.0** toolchain (`cargo +stable install <tool> --version <version> --locked --debug`). The project itself was tested with its pinned **1.85.0** toolchain. Older compatible tools (deny 0.18.2 / audit 0.21.2) could not parse modern CVSS 4 advisory records and were not counted as passes. A transient official audit-download failure succeeded on the single authorized retry. Current cargo-deny also passed with the original repository config and default transport; no transport/configuration changes are shipped.

## Corrections found during verification

- The original aggregate exposed an existing `init` bug: any ancestor `.git` directory was mistaken for a worktree. Empty protected sandbox mounts made this observable. Actual Git worktree detection fixes it; tests cover empty directories, normal repos, nested roots and linked worktree gitfiles.
- Strict Cargo ingestion exposed a trimmed diagnostic fixture missing `build-finished.success`. The required `false` field was restored, with all original diagnostics/assertions retained. A fresh real failing Cargo check independently emitted `{"reason":"build-finished","success":false}` and exited 101.
- Review found malformed MCP argument types, truncated Cargo evidence and candidate allowlist modifications could undermine gate trust. Regression tests now cover these cases.

## Boundaries

`--trusted-baseline` pins the baseline, weakening base and allowlist consistency. It does not pin every ignore rule, language pack or external tool/configuration. Saved diagnostic evidence is not a fresh run. Offline dependency checks do not establish registry existence. Optional-provider baseline entries are retained conservatively rather than automatically certified fixed. Direct-child timeout and bounded capture do not guarantee all descendant processes are terminated.

No precision/recall improvement beyond these fixtures is claimed. Native Windows/macOS execution, production editor-host integration, packaging/install/upgrade testing, full ecosystem tooling and live registry behavior remain separate validation work. The existing repository records for other platforms are historical, not results of this change.
