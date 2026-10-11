# Offline gate corpus v1

Small source-controlled synthetic fixtures, pinned by the repository revision.
`gate_regression.rs` copies them into disposable Git repositories; Git identity and
commit time are fixed. No registry, network, project toolchain, or rule engine runs.
`manifest.json` pins expected verdicts for the named change cases. Python, JavaScript,
and Unity C# are parsed as data, never executed. The fake package is deliberately
undeclared; offline results must not claim that a registry package does not exist.

Tests also cover advisory versus enforced dead code, independent diagnostic and
dependency thresholds, trusted committed baselines despite hostile working-tree
edits, invalid baseline inputs, blocked maintenance writes, and retention of
unobserved optional-provider entries. These fixtures establish regression behavior,
not real-world precision or safe-deletion guarantees.
