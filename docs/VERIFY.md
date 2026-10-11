# Trusted verification and schema 2 migration

`plumb verify` is a gate over **selected checks**, not proof that code is correct or safe to delete. Defaults execute no project tools and make no registry requests. Git is used to read the diff. A usable Git base is required, including in a new repository (make its first commit).

## Verdict and exit contract

The JSON report now has `schema_version: 2`:

| Verdict | Meaning | CLI exit |
| --- | --- | --- |
| `pass` | Selected checks completed; no new enforcing finding met its check's threshold | 0 |
| `fail` | New enforcing findings, including diff-based test weakening | 1 |
| `incomplete` | A required analysis failed, timed out, or could not run | 1 |
| `baseline-updated` | Baseline maintenance succeeded; this does **not** certify a change | 1 |

Invalid arguments, unreadable/malformed/unsupported baselines, and other request-level errors use the existing CLI error exit (2) or MCP `isError: true`, rather than a report. MCP operational analysis failure is a successful tool response containing `data.verdict: incomplete`; consumers must inspect the verdict, not just `isError`. The CLI and MCP use the same gate implementation. Finding caps truncate presentation only; verdict and totals cover all findings. `total_findings = new.len() + baselined` before truncation, including weakening.

Incomplete takes precedence over findings and maintenance. Nested diagnostics statuses are shown in `steps`, and failed/timed-out requested tools cannot silently pass. Intentionally unrequested diagnostics/rules remain visible as `skipped`; an unconfigured rule engine is optional, but a configured engine must be available. A missing/invalid Git base is incomplete, not an optional skip. `index`, `scip`, and `limitations` expose coverage, fallback, Unity, syntax and library-mode caveats.

## Per-check policy

Dead-code findings (including SCIP-backed findings) are advisory by default. The index cannot prove external uses, reflection, serialized Unity references or safe deletion. Opt in after reviewing project assumptions:

```sh
plumb verify . --lib --enforce-dead-code --fail-on high
plumb verify . --dependencies medium --weakening medium --diagnostics high --rules medium
```

`--fail-on` now controls only explicitly enforced dead code. It no longer raises every check's threshold, and `verify --fail-on none` is rejected. Other CLI commands retain their existing flags. Dependency, weakening, diagnostics and rule thresholds default independently to `medium`. MCP uses `mode: "lib"` and `policy: {"enforce_dead_code": true, "dependencies": "medium", "weakening": "medium", "diagnostics": "high", "rules": "medium"}` with `fail_on: "high"`. Unknown policy keys and invalid thresholds are rejected. Report `policy` records the effective values.

## Baselines and authority

The baseline file format remains version 1. Line-independent fingerprints and multiplicity are retained. An absent implicit `plumb-baseline.json` starts empty; an explicitly supplied missing baseline is an error except when intentionally creating it with maintenance. Relative baseline paths resolve from the project root. Malformed JSON, unsupported versions and filesystem read failures are errors. Baseline paths must remain inside the project root and symlinks are rejected.

`--update-baseline` atomically replaces the baseline only after complete analysis with no enforcing weakening finding. It never returns `pass`. Findings from the diff are never baselined. Baseline entries for optional providers are conservatively retained even if not observed, and counted in `baseline_unobserved`, not `fixed`. This avoids silently erasing diagnostics or rules during static-only maintenance. Their removal requires explicit human baseline review; automatic expiry is intentionally not claimed.

MCP baseline writes require the separate operator flag `plumb mcp --allow-baseline-update`. `--allow-exec` grants execution only and no longer grants baseline-writing authority. The agent's `update_baseline: true` cannot grant either permission. Existing integrations that used exec permission for maintenance must migrate explicitly.

### Pin a trusted baseline

A baseline editable by the change author is not a trustworthy enforcement boundary. For release/CI review, supply a full immutable Git commit ID from a trusted source:

```sh
plumb verify . --trusted-baseline <full-commit-id>
```

That mode loads `plumb-baseline.json` from the specified commit, ignores worktree baseline edits, and uses **that same commit** as the weakening diff base. The committed baseline must exist and be valid. Short hashes/branch names, `--baseline` overrides and maintenance are rejected in this mode. The candidate `.plumbgraph/allow.toml` must exactly match the pinned commit (including presence); untracked additions, deletions, changes and symlinks are rejected. MCP exposes the same `trusted_baseline` parameter.

This is deliberately a bounded trust mechanism: the operator must supply trusted thresholds and the pinned revision. It does not sandbox hostile repository content or freeze ignore files, language packs, executable tools, tool configurations or all other analysis inputs. Do not give an untrusted agent authority to change the trusted command or select its own pinned revision. Review policy/allow-list changes separately. No automatic remote fetching, persistent runners, billing changes or paid service is needed.

## Limits

Saved diagnostics are evidence supplied by the caller, not a fresh compiler run. Offline dependency checks cannot establish public registry existence. Passing optional checks requires `--run` and/or saved output; online dependency validation requires `--online`. Name-based and partially covered SCIP results retain their coverage limitations. Windows/macOS native execution and production editor-host integration need separate validation; Linux tests do not certify those platforms.

SCIP inputs must resolve within the project root, including auto-discovered indexes. Verify rejects symlinked cache/policy paths before opening them. Tool timeouts stop the direct child and bound output collection; they do not guarantee termination of every descendant process. Execute untrusted toolchains only in a suitable external sandbox.
