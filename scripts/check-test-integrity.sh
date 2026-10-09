#!/usr/bin/env bash
# Guards against silently weakening this repository's own tests.
#  1. no #[ignore] / #[should_panic] without expected= in Rust sources
#  2. PLUMB_BLESS (golden rewrite mode) must not be set in CI
#  3. the number of #[test] functions must not drop below scripts/test-count-baseline.txt
#     (raise the baseline in the same PR when you add tests; lowering it needs a stated reason)
#  4. golden files are never empty
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
err() { echo "test-integrity: $*" >&2; fail=1; }

if grep -rnE '^[[:space:]]*#\[ignore' crates --include='*.rs' >/dev/null; then
  err "found #[ignore]:"; grep -rnE '^[[:space:]]*#\[ignore' crates --include='*.rs' >&2
fi
if grep -rnE '#\[should_panic\]' crates --include='*.rs' >/dev/null; then
  err "found bare #[should_panic] (use expected = \"...\"):"; grep -rnE '#\[should_panic\]' crates --include='*.rs' >&2
fi
if [[ -n "${CI:-}" && -n "${PLUMB_BLESS:-}" ]]; then
  err "PLUMB_BLESS is set in CI; golden files must not be rewritten there"
fi

count=$(grep -rE '#\[test\]' crates --include='*.rs' | wc -l | tr -d ' ')
baseline=$(tr -d '[:space:]' < scripts/test-count-baseline.txt)
if (( count < baseline )); then
  err "test count dropped: $count < baseline $baseline"
fi

while IFS= read -r -d '' g; do
  [[ -s "$g" ]] || err "empty golden file: $g"
done < <(find crates -path '*/tests/golden/*.txt' -print0)

if (( fail )); then exit 1; fi
echo "test-integrity: OK ($count tests, baseline $baseline)"
