#!/usr/bin/env bash
# Single entry point for local and CI verification. Fails fast; prints what it runs.
# Usage: scripts/verify.sh [--quick]   (--quick skips cargo-deny / cargo-audit)
set -euo pipefail
cd "$(dirname "$0")/.."

quick=0
[[ "${1:-}" == "--quick" ]] && quick=1

step() { printf '\n==> %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --all -- --check

step "cargo clippy (-D warnings)"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "cargo test"
cargo test --workspace --locked

step "test integrity"
scripts/check-test-integrity.sh

step "dogfood: plumb verify . (offline; allow-list in .plumbgraph/allow.toml)"
cargo run --quiet --locked -p plumbgraph-cli --bin plumb -- verify .

if [[ $quick -eq 0 ]]; then
  if cargo deny --version >/dev/null 2>&1; then
    step "cargo deny check"
    cargo deny check
  else
    echo "note: cargo-deny not installed; skipping (install: cargo install cargo-deny --locked)"
  fi
  if cargo audit --version >/dev/null 2>&1; then
    step "cargo audit"
    cargo audit
  else
    echo "note: cargo-audit not installed; skipping (install: cargo install cargo-audit --locked)"
  fi
fi

printf '\nverify: OK\n'
