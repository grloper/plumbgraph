# Plumbgraph product strategy

Status: written 2026-10-10 from web research (URLs inline; every number below is quoted from the
cited page, not measured by us). Companion: [VALIDATION.md](VALIDATION.md) (what actually works today).
Sections marked **[to be revised after validation]** depend on the measured results.

## 1. What the evidence says agents really fail at

| Failure | Evidence (cited) | Implication |
|---|---|---|
| **Hallucinated packages** | USENIX Security 2025: 16 LLMs, 576k Python/JS samples, 2.23M package recs, **19.7 % hallucinated**, 205 474 unique fake names; commercial 5.2 % vs open-source 21.7 %. https://www.usenix.org/system/files/usenixsecurity25-spracklen.pdf ; attack framing ("slopsquatting") https://socket.dev/blog/slopsquatting-how-ai-hallucinations-are-fueling-a-new-class-of-supply-chain-attacks | Concrete, measurable, security-relevant. A manifest-vs-import + registry check is a real product. |
| **Gaming the tests** (delete/skip/loosen tests, special-case inputs) | ImpossibleBench (ICLR 2026): agents delete/modify tests, special-case, overload operators; https://arxiv.org/html/2510.20270 ; EvilGenie: agents edit/ignore tests even on solvable tasks https://arxiv.org/html/2511.21654v2 | A diff-level "did tests get weaker" check addresses the *test-edit* subset (not special-casing). Be explicit about that limit. |
| **Exploration cost / localization** | SWE-Explore (848 issues, 10 languages): file-level localization is already strong; *line-level coverage and ranking* separate good from bad explorers; agentic explorers beat classical retrieval. https://arxiv.org/html/2606.07297v1 . FastContext: dedicated explorer gives up to +5.5 % resolution and up to −60 % tokens. https://arxiv.org/abs/2606.14066 | Static repo maps compete in the *weakest* tier (classical retrieval) and file-level is not the bottleneck. Do not make "better map" the headline. |
| **Context files do not reliably help** | "Evaluating AGENTS.md…": context files **reduced** success ~0.5–2 % and raised cost >20 %; human-written files ~+4 % on AGENTbench. https://www.emergentmind.com/papers/2602.11988 | `plumb init` must write a *minimal* block. Verbose injected context is a liability. |
| **IDE-style tools are not automatically a win** | Independent test of Serena with Codex: 20/20 accepted both ways, but +24 % time, +40 % tokens, +112 % tool actions. https://dev.to/b2a48b/ide-style-code-navigation-and-refactoring-dont-automatically-improve-your-coding-agent-28n . Counter-data point (vendor-adjacent, single project): ManoMano Java refactor with Serena finished with all tests passing where vanilla/LSP did not. https://medium.com/manomano-tech/project-aegis-benchmarking-ai-agents-and-why-serena-is-our-new-must-have-311673db35dd | Navigation tools help on large cross-file refactors, can cost on routine tasks. We must publish our own with/without numbers. |
| **Graph retrieval helps modestly** | RepoGraph plug-in: +2.34 to +2.66 points resolve rate on SWE-bench Lite https://arxiv.org/pdf/2410.14684 ; LocAgent graph-guided localization Acc@5 ≈ 92–94 % file-level https://arxiv.org/pdf/2503.09089 ; commit-history memory +4.9 abs Acc@5 https://arxiv.org/pdf/2510.01003 | Graph signals are worth single-digit points. Not a moat. |
| **Humans over-trust AI output** | METR RCT: 16 experienced OSS devs, 246 tasks, **19 % slower** with AI, believed 20 % faster. https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/ (applies to mature familiar repos, early-2025 tools) | Review/verification cost is where time is lost; an automatic, low-noise gate has value. |

## 2. Competitors and standards (and their real weaknesses)

| Player | Strength | Weakness relevant to us |
|---|---|---|
| Aider repo map (https://aider.chat/docs/repomap.html) | Mature tree-sitter + PageRank map, 1k-token default | Context only; no verification; name-based edges. |
| Serena (https://github.com/oraios/serena) | LSP-backed symbol find/refs/edit, 40+ languages | Needs language servers; independent test shows token/time overhead; editing focus, not gating. |
| CodeGraph (https://colbymchenry.github.io/codegraph/) | Tree-sitter graph, MCP, impact analysis, wide agent auto-config | Same space as our map/impact; self-reported benchmarks. Direct competitor on the *context* half. |
| Sourcegraph/SCIP (https://github.com/scip-code/scip) | Compiler-grade precise index | Needs per-language indexers + build; enterprise pricing for hosted. We consume SCIP, not compete. |
| Semgrep (https://semgrep.dev, MCP via `semgrep mcp`) | 30+ languages, no build; community edition mostly single-function | Not about agent-specific failures (fake deps, weakened tests, dead code). Complement; we shell out. |
| CodeQL | Deep data-flow, fewer false positives | Needs buildable project for compiled languages; slow; security only. |
| knip / vulture / `go deadcode` | Best-in-class dead code per language | Single language each. We are the polyglot, zero-config, *diff-aware* layer, so we must stay honest about precision vs these. |

## 3. The single wedge

> **Plumbgraph is the pre-submit gate for AI-written diffs: it fails the change when the agent
> introduced a package that does not exist, weakened a test, or left new dead code — and says
> nothing otherwise.**

Why this and not "code intelligence / better map":
1. Evidence for the failure modes is strong and measurable (19.7 % fake packages; test tampering benchmarks).
2. Evidence that *context* tools move success rates is weak (AGENTS.md negative result; repo-graph +2–3 pts; Serena overhead).
3. Competitors crowd the map/impact space (CodeGraph, Aider, Serena, code-review-graph); nobody owns "new-only, provenance-tagged verdict".
4. A gate is binary and benchmarkable: precision/recall on seeded faults, false-positive rate on clean diffs.

`map`/`impact` stay as secondary tools that help the agent *and* feed the gate (tests-to-run), but are not the pitch.

**Target user:** developers running Claude Code / Codex / Cursor in autonomous or semi-autonomous mode on
Python/JS/TS/Rust/Go/Java/C# repos who review PRs written by agents (solo maintainers and small teams first).

## 4. Distribution

1. **GitHub Action + pre-commit hook** (primary — works with *any* agent, produces a visible PR check; `verify` already has exit codes and baseline). Needs: prebuilt binaries (not yet published).
2. **Prebuilt binaries + `cargo install`/`npx`/`pipx` wrappers** — install friction is the first thing that kills adoption; nothing is published today.
3. **Claude Code plugin/marketplace** — git-hosted `.claude-plugin/marketplace.json`; ship a Stop/PostToolUse hook that runs `plumb verify`. https://code.claude.com/docs/en/plugin-marketplaces
4. **Official MCP Registry** (`server.json`, `mcp-publisher publish`; npm package must set `mcpName`). https://github.com/modelcontextprotocol/registry
5. Cursor rules / AGENTS.md snippet (minimal, per the AGENTS.md study).

## 5. Success metrics (all measurable)

| Metric | Target for "wedge proven" |
|---|---|
| Hallucinated-dep recall on seeded fakes (py/js/rust, ≥100 per ecosystem) | ≥ 95 % |
| Hallucinated-dep false positives on 20 real repos at HEAD | ≤ 1 per 1 000 imports; 0 high-confidence |
| Test-weakening recall on seeded commits (delete/skip/loosen, 7 languages) | ≥ 90 % |
| Test-weakening false positives on 200 real human commits that touch tests | ≤ 5 % flagged high |
| `verify` false-fail rate on clean agent diffs (50 diffs) | ≤ 2 % |
| Wall time `verify` on a 10k-file repo, warm | ≤ 10 s |
| Agent A/B (SWE-bench-style 50 tasks, same agent/model, with vs without gate) | gate catches ≥ 1 real defect per 10 tasks; resolve rate not lower (CI-bounded) |
| Adoption | 500 weekly Action runs / 200 GitHub stars within 90 days of first binary release |

## 6. Benchmark plan

1. **Seeded-fault suite** (offline, deterministic, CI-able): mutate real repos — fake imports, deleted/skipped/loosened tests, unused functions — and score recall/precision per command. Started in docs/validation (see VALIDATION.md).
2. **Clean-diff false-positive suite**: replay last N merged human PRs on 20 repos through `verify`; every flag hand-labelled.
3. **Agent A/B**: SWE-bench Verified/Multilingual subset (≥50 tasks), fixed model and scaffold, arms = baseline / +plumb MCP / +plumb gate. Metrics: resolve rate, tokens, tool calls, reward-hack rate on ImpossibleBench-style tasks (https://github.com/safety-research/impossiblebench). Report CIs; no claim without n ≥ 50.
4. Publish raw logs; accept null results.

## 7. Roadmap with kill criteria

| Milestone | Scope | Exit criteria | Kill / pivot criterion |
|---|---|---|---|
| M0 (now → +2 wk) | Fix validation blockers; green CI; no unverified claims | Every `fail` in VALIDATION.md for the gated commands closed or documented | — |
| M1 (+4 wk) | Prebuilt binaries (Linux/macOS/Windows), GitHub Action, pre-commit; v1.0.0 only after M0+M1 | Action runs on 5 external repos; install <1 min | If install success <90 % on a 10-person dogfood, stop feature work until fixed |
| M2 (+8 wk) | Seeded-fault suite + clean-diff FP suite published | Metrics in §5 met for deps & weakening | If dep or weakening FP > 5 % high-confidence after two tuning rounds → demote that check to "advisory" |
| M3 (+12 wk) | Claude Code plugin + MCP Registry; A/B on ≥50 tasks | Gate catches ≥1 defect/10 tasks, resolve rate not lower | If A/B shows no defects caught and ≥10 % extra tokens → drop MCP context tools, ship gate-only |
| M4 (+6 mo) | Dead-code precision work via SCIP/knip/vulture providers | Dead-code precision ≥ 90 % on 10 repos hand-sampled | If <70 % precision, remove dead-code from the gate default |
| Overall | 90 days after first binary: <100 weekly Action runs and <50 stars → stop or fold into an existing project (e.g. Semgrep rule pack) |

## 8. Risks

* **Platform risk:** Claude Code/Codex/Cursor ship native "did tests weaken / package exists" checks. Mitigation: be agent-agnostic, CI-resident, open.
* **Precision risk:** dead-code on name-based edges produces false positives; one bad demo kills trust (the owner's "look like fools" concern). Mitigation: confidence tiers, default gate on high-confidence only, published FP rates.
* **Evidence risk:** context tools may not help (see §1). Mitigation: wedge is the gate, A/B gates the MCP investment.
* **Supply-chain/security risk:** a tool that runs `--run` on untrusted repos. Mitigation: exec off by default (already).
* **Crowding:** CodeGraph (73k stars per docs/LANDSCAPE.md snapshot) owns mindshare in map/MCP. Do not fight there.
* **Registry network dependence:** `check-deps --online` needs network and rate limits; offline mode must stay useful.
