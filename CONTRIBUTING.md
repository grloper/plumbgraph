# Contributing to Plumbgraph

Thanks for helping! Plumbgraph is pre-alpha; small, well-tested changes are easiest to review.

## Ground rules

1. **Only claim what tests prove.** New behaviour needs tests (unit and, where it affects findings, a fixture + golden file). Documentation must not claim precision/recall that has not been measured.
2. **Every finding has a `source` and a `confidence`.** Keep it that way.
3. **No weakening tests to get green.** `scripts/check-test-integrity.sh` fails on `#[ignore]`, bare `#[should_panic]`, or a drop in the number of tests. If you really must lower `scripts/test-count-baseline.txt`, say why in the PR.
4. Prefer small PRs; open an issue first for anything that changes output formats or the MCP tool contract.

## Setup

```bash
git clone https://github.com/grloper/plumbgraph && cd plumbgraph
scripts/verify.sh            # fmt, clippy -D warnings, tests, integrity, cargo-deny/audit if installed
scripts/verify.sh --quick    # skip deny/audit
```

Rust toolchain: see `rust-toolchain.toml` (MSRV 1.85). Optional: `pipx install pre-commit && pre-commit install`.

## Golden tests

Dead-code and dependency fixtures live in `crates/core/tests/fixtures/` (manifests are stored as `*.fixture` so GitHub/Dependabot do not scan them). Expected output is in `crates/core/tests/golden/`. After an intentional change:

```bash
PLUMB_BLESS=1 cargo test -p plumbgraph-core --test deadcode_golden --test deps_golden
git diff crates/core/tests/golden      # review every changed line by hand
```

## Adding a language pack

See [docs/PACKS.md](docs/PACKS.md). Packs are `pack.toml` + tree-sitter queries; v0.1 can only select one of the built-in grammars.

## Sign-off (DCO)

Sign your commits with `git commit -s` to certify the [Developer Certificate of Origin](https://developercertificate.org/). Contributions are licensed under Apache-2.0.

## Code of conduct

Participation is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
