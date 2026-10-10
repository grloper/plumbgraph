//! The `plumb` command line.

use anyhow::Result;
use clap::{Parser, Subcommand};
use plumbgraph_core::diagnostics::Format;
use plumbgraph_core::ops::{
    self, DeadCodeParams, DepsParams, DiagInput, DiagParams, ScipOpts, Target,
};
use plumbgraph_core::{Finding, Level, Severity};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "plumb",
    version,
    about = "Know your code. Verify the change.",
    long_about = "Plumbgraph: tier-0 (tree-sitter) code intelligence and pre-submit checks for AI coding agents.\nResults are heuristic unless a SCIP index covers the code; every finding carries a source and a confidence."
)]
struct Cli {
    /// SQLite index location (default: <path>/.plumbgraph/index.db)
    #[arg(long, global = true, value_name = "FILE")]
    db: Option<PathBuf>,
    /// Machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum FailOn {
    None,
    Low,
    Medium,
    High,
}

impl FailOn {
    fn tripped(self, findings: &[Finding]) -> bool {
        let min = match self {
            FailOn::None => return false,
            FailOn::Low => Level::Low,
            FailOn::Medium => Level::Medium,
            FailOn::High => Level::High,
        };
        findings.iter().any(|f| f.level >= min)
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Build or refresh the code graph for a project directory
    Index {
        path: PathBuf,
        /// Re-extract every file
        #[arg(long)]
        force: bool,
    },
    /// Report unused code with confidence levels
    #[command(
        after_help = "Unity (C#): engine callbacks, partial classes, scenes/prefabs and other limits are documented in docs/PACKS.md#unity-c (https://github.com/grloper/plumbgraph/blob/main/docs/PACKS.md#unity-c). Do not mass-delete HIGH findings under Assets/ without checking."
    )]
    DeadCode {
        path: PathBuf,
        /// Treat exported symbols as public API (entry points)
        #[arg(long)]
        lib: bool,
        /// Minimum confidence to report (0..1). Default 0.70 hides `low`.
        #[arg(long, default_value_t = 0.70)]
        min_confidence: f64,
        /// Show low-confidence findings too (same as --min-confidence 0)
        #[arg(long)]
        include_low: bool,
        /// Symbol kinds to report (repeatable). Default: function, method, class, struct, enum, trait, interface, type, const
        #[arg(long = "kind")]
        kinds: Vec<String>,
        /// Extra allow-list TOML (in addition to <path>/.plumbgraph/allow.toml)
        #[arg(long)]
        allow_file: Option<PathBuf>,
        /// Do not report symbols used only by tests
        #[arg(long)]
        no_test_only: bool,
        /// Print debug notes (e.g. `unity-message@partial-merged`: a message suppressed by merging partial class parts)
        #[arg(long)]
        verbose: bool,
        /// Only report files under this project-relative prefix
        #[arg(long)]
        only: Option<String>,
        /// Print at most N findings (highest confidence first); 0 = all. Totals and the exit code always cover every finding.
        #[arg(long, default_value_t = 200)]
        max_findings: usize,
        /// Exit with status 1 when a finding of at least this level exists
        #[arg(long, value_enum, default_value = "none")]
        fail_on: FailOn,
        #[command(flatten)]
        scip: ScipArgs,
    },
    /// Check imports against manifests and package registries
    CheckDeps {
        path: PathBuf,
        /// Do not contact any registry (manifest + resolution checks only)
        #[arg(long)]
        offline: bool,
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f64,
        /// Only look up undeclared imports, not every declared dependency
        #[arg(long)]
        no_check_declared: bool,
        #[arg(long)]
        only: Option<String>,
        /// Print at most N findings (highest confidence first); 0 = all. The exit code always covers every finding.
        #[arg(long, default_value_t = 200)]
        max_findings: usize,
        #[arg(long, value_enum, default_value = "none")]
        fail_on: FailOn,
    },
    /// Detect test weakening in `git diff <base>` (working tree vs base, or base..head)
    Weakening {
        /// Revision to compare against (branch, tag or commit)
        #[arg(long, default_value = "HEAD")]
        base: String,
        #[arg(long)]
        head: Option<String>,
        /// Repository directory
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long, value_enum, default_value = "none")]
        fail_on: FailOn,
    },
    /// Find symbol definitions by name
    Find {
        path: PathBuf,
        query: String,
        #[arg(long = "kind")]
        kinds: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show incoming references to a symbol
    Refs {
        path: PathBuf,
        symbol: String,
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f64,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        scip: ScipArgs,
    },
    /// Normalised compiler / type-checker / linter / LSP diagnostics.
    ///
    /// `--from FORMAT:FILE` ingests saved output and executes nothing. `--run` and `--lsp`
    /// execute the project's toolchain (cargo check runs build scripts and proc-macros): use
    /// them only on code you trust. FORMAT is cargo-json, ruff-json, pyright-json or tsc; FILE
    /// may be `-` for stdin.
    Diagnostics {
        path: PathBuf,
        /// Saved tool output to ingest, `FORMAT:FILE` (repeatable)
        #[arg(long = "from", value_name = "FORMAT:FILE")]
        from: Vec<String>,
        /// Run the applicable tools: cargo check + clippy (Cargo.toml), tsc (tsconfig.json), ruff + pyright (Python project files)
        #[arg(long)]
        run: bool,
        /// With --run: only these tools (cargo-check, clippy, tsc, ruff, pyright)
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// Start this language server (command line, split on spaces) and collect publishDiagnostics (repeatable)
        #[arg(long = "lsp", value_name = "COMMAND")]
        lsp: Vec<String>,
        /// Files to open in the language server (default: up to 100 source files under PATH)
        #[arg(long = "lsp-file")]
        lsp_files: Vec<PathBuf>,
        /// Per-tool / LSP timeout in seconds
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
        /// Hide diagnostics below this severity
        #[arg(long, value_enum, default_value = "warning")]
        min_severity: SevArg,
        /// Exit with status 1 when a diagnostic of at least this confidence level exists
        #[arg(long, value_enum, default_value = "none")]
        fail_on: FailOn,
    },
    /// Show how well SCIP index files cover the project (reads index.scip, executes nothing)
    Scip {
        path: PathBuf,
        #[command(flatten)]
        scip: ScipArgs,
    },
    /// Compact, token-budgeted repo map: most important symbols first (PageRank over resolved edges)
    Map {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Hard token budget (estimated as chars/4)
        #[arg(long, visible_alias = "budget", default_value_t = 1500)]
        tokens: usize,
        /// Boost files changed vs this git revision (use `--changed` alone for HEAD)
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD", value_name = "REV")]
        changed: Option<String>,
        /// Boost these project-relative files (repeatable)
        #[arg(long = "focus")]
        focus: Vec<String>,
        #[arg(long)]
        include_tests: bool,
        #[command(flatten)]
        scip: ScipArgs,
    },
    /// What breaks if a symbol (or the current diff) changes: transitive callers and tests to run
    Impact {
        /// Symbol name or qualified name; omit to analyse the working-tree diff
        symbol: Option<String>,
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Diff base when no symbol is given
        #[arg(long, default_value = "HEAD")]
        base: String,
        #[arg(long, default_value_t = 4)]
        depth: u32,
        #[arg(long, default_value_t = 0.3)]
        min_confidence: f64,
        #[command(flatten)]
        scip: ScipArgs,
    },
    /// One pass/fail gate: dead code, dependency hallucination, test weakening, diagnostics, rule engines.
    ///
    /// Only findings that are *new* relative to the baseline (`plumb-baseline.json`) fail.
    /// Nothing is executed unless `--run` is given (diagnostics tools, semgrep, ast-grep).
    Verify {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Diff base for test-weakening (working tree vs this revision)
        #[arg(long, default_value = "HEAD")]
        base: String,
        /// Execute external tools (cargo check/clippy, tsc, ruff, pyright, semgrep, ast-grep). Only on trusted code.
        #[arg(long)]
        run: bool,
        /// Saved diagnostics output to merge in, `FORMAT:FILE` (executes nothing)
        #[arg(long = "from", value_name = "FORMAT:FILE")]
        from: Vec<String>,
        /// Look up packages on PyPI/npm/crates.io (sends package names over the network)
        #[arg(long)]
        online: bool,
        /// Baseline file (default <path>/plumb-baseline.json)
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Write the current findings as the new baseline (exit 0)
        #[arg(long)]
        update_baseline: bool,
        /// Print at most N new findings (highest confidence first); 0 = all. Verdict and counts always cover every finding.
        #[arg(long, default_value_t = 200)]
        max_findings: usize,
        /// Fail when a new finding of at least this level exists
        #[arg(long, value_enum, default_value = "medium")]
        fail_on: FailOn,
        /// semgrep config (file, dir or registry pack such as p/ci; registry packs need network)
        #[arg(long)]
        semgrep_config: Option<String>,
        #[arg(long)]
        lib: bool,
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
        #[command(flatten)]
        scip: ScipArgs,
    },
    /// List installed providers (SCIP indexers, diagnostics tools, semgrep, ast-grep, LSPs). Executes nothing.
    Doctor {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Run installed SCIP indexers to sharpen the graph (writes .plumbgraph/scip/*.scip). Executes tools: trusted code only.
    Enrich {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Indexer ids (see `plumb doctor`); default: every installed indexer relevant to the project
        #[arg(long = "only")]
        only: Vec<String>,
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
    },
    /// Write AGENTS.md instructions and MCP config for Claude Code / Cursor (prints the Codex snippet)
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// claude, cursor, codex or all (default: whatever is detected)
        #[arg(long = "agent")]
        agents: Vec<String>,
        /// Show what would be written (including the .gitignore lines) without writing
        #[arg(long)]
        dry_run: bool,
        /// Do not add `.plumbgraph/*` and `!.plumbgraph/allow.toml` to .gitignore
        #[arg(long)]
        no_gitignore: bool,
    },
    /// Serve the tools over MCP (stdio, JSON-RPC)
    Mcp {
        /// Project root the tools are confined to (default: current directory)
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Let the `diagnostics` tool run project toolchains and language servers when the
        /// agent asks. Off by default: a repository must not be able to enable this itself.
        #[arg(long)]
        allow_exec: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SevArg {
    Info,
    Warning,
    Error,
}

#[derive(clap::Args, Clone)]
struct ScipArgs {
    /// SCIP index file to use (repeatable). Default: auto-detect <path>/index.scip and <path>/.plumbgraph/index.scip
    #[arg(long = "scip", value_name = "FILE")]
    scip: Vec<PathBuf>,
    /// Ignore SCIP indexes; name-based analysis only
    #[arg(long)]
    no_scip: bool,
}

impl ScipArgs {
    fn opts(&self) -> ScipOpts {
        ScipOpts {
            paths: self.scip.clone(),
            disable: self.no_scip,
        }
    }
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

/// Highest-confidence-first view of at most `max` findings (0 = all) plus how many were left out.
fn capped(findings: &[Finding], max: usize) -> (Vec<&Finding>, usize) {
    let mut v: Vec<&Finding> = findings.iter().collect();
    if max == 0 || v.len() <= max {
        return (v, 0);
    }
    v.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let hidden = v.len() - max;
    v.truncate(max);
    (v, hidden)
}

fn note_hidden(hidden: usize) {
    if hidden > 0 {
        println!("\n... {hidden} more finding(s) not shown (lowest confidence last); use --max-findings 0 to print all, or --only <path> to narrow down");
    }
}

/// JSON output with `findings` capped; `summary`/totals still describe everything and a
/// `truncated` object says what was left out.
fn print_json_capped<T: serde::Serialize>(r: &T, max: usize) -> Result<()> {
    print_json_capped_key(r, max, "findings")
}

fn print_json_capped_key<T: serde::Serialize>(r: &T, max: usize, key: &str) -> Result<()> {
    let mut v = serde_json::to_value(r)?;
    if let Some(arr) = v.get_mut(key).and_then(|f| f.as_array_mut()) {
        let total = arr.len();
        if max > 0 && total > max {
            arr.sort_by(|a, b| {
                let c = |x: &serde_json::Value| x["confidence"].as_f64().unwrap_or(0.0);
                c(b).partial_cmp(&c(a)).unwrap_or(std::cmp::Ordering::Equal)
            });
            arr.truncate(max);
            v["truncated"] = serde_json::json!({
                "shown": max,
                "total": total,
                "hint": "output capped, lowest confidence findings omitted; pass --max-findings 0 for everything or --only <path> to narrow down"
            });
        }
    }
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

fn render(f: &Finding) {
    let loc = match f.column {
        Some(c) => format!("{}:{}:{}", f.file, f.line, c),
        None => format!("{}:{}", f.file, f.line),
    };
    let rule = if f.category == "diagnostic" {
        format!("{} ", f.rule)
    } else {
        String::new()
    };
    println!(
        "{:<6} {:.2}  {}  {}{}  [{}]",
        f.level.as_str().to_uppercase(),
        f.confidence,
        loc,
        rule,
        f.message,
        f.source
    );
    for e in &f.evidence {
        println!("         why:  {e}");
    }
    for r in &f.fp_risks {
        println!("         risk: {r}");
    }
}

fn run(cli: Cli) -> Result<bool> {
    let json = cli.json;
    let target = |p: PathBuf| Target {
        root: p,
        db: cli.db.clone(),
    };
    match cli.cmd {
        Cmd::Index { path, force } => {
            let st = ops::index(&target(path), force)?;
            if json {
                print_json(&st)?;
            } else {
                println!(
                    "indexed {} file(s): parsed {}, unchanged {}, removed {}",
                    st.files_seen, st.files_parsed, st.files_unchanged, st.files_removed
                );
                println!(
                    "symbols {}, references {}, imports {}, edges {}",
                    st.symbols, st.refs, st.imports, st.edges
                );
                let langs: Vec<String> = st
                    .by_language
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                println!("languages: {}", langs.join(" "));
                if st.files_with_syntax_errors + st.parse_errors + st.files_skipped_large > 0 {
                    println!("warnings: {} file(s) with syntax errors, {} parse error(s), {} skipped (too large)", st.files_with_syntax_errors, st.parse_errors, st.files_skipped_large);
                }
                println!("db: {}", st.db_path);
            }
            Ok(false)
        }
        Cmd::DeadCode {
            path,
            lib,
            min_confidence,
            include_low,
            kinds,
            allow_file,
            no_test_only,
            verbose,
            only,
            fail_on,
            max_findings,
            scip,
        } => {
            let p = DeadCodeParams {
                library_mode: lib,
                min_confidence: if include_low { 0.0 } else { min_confidence },
                kinds: if kinds.is_empty() { None } else { Some(kinds) },
                allow_file,
                path_prefix: only,
                include_test_only: !no_test_only,
                scip: scip.opts(),
            };
            let r = ops::run_dead_code(&target(path), &p)?;
            if json {
                print_json_capped(&r, max_findings)?;
            } else {
                let (shown, hidden) = capped(&r.findings, max_findings);
                for f in &shown {
                    render(f);
                }
                note_hidden(hidden);
                if verbose {
                    for n in &r.notes {
                        println!("debug: {n}");
                    }
                }
                println!(
                    "\n{} finding(s): {} high, {} medium, {} low  (indexed {} files, {} symbols)",
                    r.summary.total,
                    r.summary.high,
                    r.summary.medium,
                    r.summary.low,
                    r.index.files_seen,
                    r.index.symbols
                );
                match &r.scip {
                    Some(st) => println!(
                        "scip: {} document(s) matched of {}, {} symbol(s) resolved precisely, {} stale file(s) analysed name-based; findings marked [scip] come from the index, the rest are name-based.",
                        st.documents_matched, st.documents, st.symbols_mapped, st.stale_files
                    ),
                    None => println!("note: tier-0 name-based analysis (no SCIP index); verify medium/low findings before deleting anything."),
                }
                if let Some(l) = r.limitations.iter().find(|l| l.contains("--lib")) {
                    println!("note: {l}");
                }
                for l in [ops::UNITY_PACK_NOTE, ops::UNITY_MASS_DELETE_NOTE] {
                    if r.limitations.iter().any(|x| x == l) {
                        println!("note: {l}");
                    }
                }
            }
            Ok(fail_on.tripped(&r.findings))
        }
        Cmd::CheckDeps {
            path,
            offline,
            min_confidence,
            no_check_declared,
            only,
            fail_on,
            max_findings,
        } => {
            let p = DepsParams {
                offline,
                path_prefix: only,
                min_confidence,
                check_declared: !no_check_declared,
            };
            let r = ops::run_check_deps(&target(path), &p, None)?;
            if json {
                print_json_capped(&r, max_findings)?;
            } else {
                let (shown, hidden) = capped(&r.findings, max_findings);
                for f in &shown {
                    render(f);
                }
                note_hidden(hidden);
                let unknown = r
                    .lookups
                    .iter()
                    .filter(|l| l.result.starts_with("unknown"))
                    .count();
                println!(
                    "\n{} finding(s); {} import(s) checked, {} external; registry: {}{}",
                    r.findings.len(),
                    r.imports_checked,
                    r.external_imports,
                    r.registry,
                    if r.offline {
                        " (offline: manifests and local resolution only)"
                    } else {
                        ""
                    }
                );
                println!(
                    "registry lookups: {} ({} inconclusive)",
                    r.lookups.len(),
                    unknown
                );
            }
            Ok(fail_on.tripped(&r.findings))
        }
        Cmd::Weakening {
            base,
            head,
            repo,
            fail_on,
        } => {
            let r = ops::run_weakening(&repo, &base, head.as_deref())?;
            if json {
                print_json(&r)?;
            } else {
                for f in &r.findings {
                    render(f);
                }
                println!(
                    "\n{} finding(s) in {} changed file(s) vs {} ({})",
                    r.findings.len(),
                    r.files_changed,
                    r.base,
                    &r.base_commit[..r.base_commit.len().min(12)]
                );
                for n in &r.notes {
                    println!("note: {n}");
                }
            }
            Ok(fail_on.tripped(&r.findings))
        }
        Cmd::Find {
            path,
            query,
            kinds,
            limit,
        } => {
            let hits = ops::find_symbol(&target(path), &query, &kinds, limit)?;
            if json {
                print_json(&hits)?;
            } else {
                for h in &hits {
                    println!(
                        "{:<9} {}  {}:{}{}",
                        h.kind,
                        h.qname,
                        h.file,
                        h.line,
                        if h.exported { "  (exported)" } else { "" }
                    );
                }
                println!("{} hit(s)", hits.len());
            }
            Ok(false)
        }
        Cmd::Refs {
            path,
            symbol,
            min_confidence,
            limit,
            scip,
        } => {
            let hits =
                ops::references_scip(&target(path), &symbol, min_confidence, limit, &scip.opts())?;
            if json {
                print_json(&hits)?;
            } else {
                for h in &hits {
                    println!(
                        "{:.2} {:<6} {}:{}  in {}  -> {} ({}:{})",
                        h.confidence,
                        h.kind,
                        h.file,
                        h.line,
                        h.from,
                        h.target,
                        h.target_file,
                        h.target_line
                    );
                }
                println!("{} reference(s)", hits.len());
            }
            Ok(false)
        }
        Cmd::Diagnostics {
            path,
            from,
            run,
            tools,
            lsp,
            lsp_files,
            timeout_secs,
            min_severity,
            fail_on,
        } => {
            let mut inputs = vec![];
            for spec in &from {
                let (fmt, file) = spec
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("--from expects FORMAT:FILE, got `{spec}`"))?;
                let format: Format = fmt.parse()?;
                let text = if file == "-" {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                    s
                } else {
                    std::fs::read_to_string(file)
                        .map_err(|e| anyhow::anyhow!("reading {file}: {e}"))?
                };
                inputs.push(DiagInput {
                    format,
                    label: file.to_string(),
                    text,
                });
            }
            let p = DiagParams {
                inputs,
                run,
                tools,
                lsp: lsp
                    .iter()
                    .map(|c| c.split_whitespace().map(String::from).collect())
                    .collect(),
                // relative file names are resolved against the project root by the LSP client
                lsp_files,
                timeout: std::time::Duration::from_secs(timeout_secs),
                min_severity: match min_severity {
                    SevArg::Info => Severity::Info,
                    SevArg::Warning => Severity::Warning,
                    SevArg::Error => Severity::Error,
                },
            };
            let r = ops::run_diagnostics(&path, &p)?;
            if json {
                print_json(&r)?;
            } else {
                for f in &r.findings {
                    render(f);
                }
                println!("\n{} diagnostic(s)", r.findings.len());
                for t in &r.tools {
                    println!(
                        "  {:<14} {:<9} {} finding(s){}",
                        t.tool,
                        t.status,
                        t.findings,
                        t.note
                            .as_ref()
                            .map(|n| format!("  ({n})"))
                            .unwrap_or_default()
                    );
                }
                if r.dropped_outside_root > 0 {
                    println!(
                        "note: {} diagnostic(s) outside the project root were dropped",
                        r.dropped_outside_root
                    );
                }
                println!(
                    "{}",
                    if r.executed {
                        "note: external tools were executed for this report."
                    } else {
                        "note: nothing was executed; diagnostics come from saved output."
                    }
                );
            }
            Ok(fail_on.tripped(&r.findings))
        }
        Cmd::Scip { path, scip } => {
            let st = ops::scip_status(&target(path), &scip.opts())?;
            if json {
                print_json(
                    &st.clone()
                        .map(|s| serde_json::to_value(s).unwrap_or_default())
                        .unwrap_or(serde_json::json!({"found": false})),
                )?;
            } else {
                match st {
                    None => println!("no SCIP index found (looked for index.scip and .plumbgraph/index.scip); analysis is name-based"),
                    Some(s) => {
                        println!("indexes: {}", s.indexes.join(", "));
                        println!("tools: {}", s.tools.join(", "));
                        println!(
                            "documents {} (matched {}, not in the project {}, stale {}); symbols resolved {}; edges replaced {} / added {}",
                            s.documents, s.documents_matched, s.documents_unmatched, s.stale_files, s.symbols_mapped, s.edges_replaced, s.edges_added
                        );
                    }
                }
            }
            Ok(false)
        }
        Cmd::Map {
            path,
            tokens,
            changed,
            focus,
            include_tests,
            scip,
        } => {
            let r = plumbgraph_core::map::run_map(
                &target(path),
                &plumbgraph_core::map::MapParams {
                    tokens,
                    changed,
                    focus,
                    include_tests,
                    scip: scip.opts(),
                },
            )?;
            if json {
                print_json(&r)?;
            } else {
                print!("{}", r.text);
            }
            Ok(false)
        }
        Cmd::Impact {
            symbol,
            path,
            base,
            depth,
            min_confidence,
            scip,
        } => {
            use plumbgraph_core::map::{run_impact, ImpactParams, ImpactTarget};
            let r = run_impact(
                &target(path),
                &ImpactParams {
                    target: match symbol {
                        Some(s) => ImpactTarget::Symbol(s),
                        None => ImpactTarget::Diff(base),
                    },
                    depth,
                    min_confidence,
                    scip: scip.opts(),
                },
            )?;
            if json {
                print_json(&r)?;
            } else {
                println!("changed: {}", r.seeds.join(", "));
                for a in &r.affected {
                    println!(
                        "d{} {:.2} {:<8} {}  {}:{}  via {}{}  [{}]",
                        a.distance,
                        a.confidence,
                        a.kind,
                        a.qname,
                        a.file,
                        a.line,
                        a.via,
                        if a.is_test { "  (test)" } else { "" },
                        a.source
                    );
                }
                println!(
                    "
{} affected symbol(s) in {} file(s); module-level dependents: {}",
                    r.affected.len(),
                    r.affected_files.len(),
                    r.module_level_dependents.len()
                );
                if !r.tests_to_run.is_empty() {
                    println!("tests to run: {}", r.tests_to_run.join(" "));
                }
                for l in &r.limitations {
                    println!("note: {l}");
                }
            }
            Ok(false)
        }
        Cmd::Verify {
            path,
            base,
            run,
            from,
            online,
            baseline,
            update_baseline,
            fail_on,
            max_findings,
            semgrep_config,
            lib,
            timeout_secs,
            scip,
        } => {
            use plumbgraph_core::verify::{run_verify, VerifyParams};
            let mut inputs = vec![];
            for spec in &from {
                let (fmt, file) = spec
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("--from expects FORMAT:FILE, got `{spec}`"))?;
                let format: Format = fmt.parse()?;
                let text = std::fs::read_to_string(file)
                    .map_err(|e| anyhow::anyhow!("reading {file}: {e}"))?;
                inputs.push(DiagInput {
                    format,
                    label: file.to_string(),
                    text,
                });
            }
            let r = run_verify(
                &target(path),
                &VerifyParams {
                    base,
                    run,
                    online_deps: online,
                    diag_inputs: inputs,
                    baseline,
                    update_baseline,
                    fail_on: match fail_on {
                        FailOn::None | FailOn::Low => Level::Low,
                        FailOn::Medium => Level::Medium,
                        FailOn::High => Level::High,
                    },
                    semgrep_config,
                    timeout: std::time::Duration::from_secs(timeout_secs),
                    scip: scip.opts(),
                    library_mode: lib,
                },
            )?;
            if json {
                print_json_capped_key(&r, max_findings, "new")?;
            } else {
                let (shown, hidden) = capped(&r.new, max_findings);
                for f in &shown {
                    render(f);
                }
                note_hidden(hidden);
                println!();
                for s in &r.steps {
                    println!(
                        "  {:<16} {:<8} {} finding(s){}",
                        s.name,
                        s.status,
                        s.findings,
                        s.note
                            .as_ref()
                            .map(|n| format!("  ({n})"))
                            .unwrap_or_default()
                    );
                }
                println!(
                    "
verify: {}  ({} new, {} baselined, {} fixed since baseline; fail-on {})",
                    if matches!(fail_on, FailOn::None) {
                        "REPORT-ONLY".to_string()
                    } else {
                        r.verdict.to_uppercase()
                    },
                    r.new.len(),
                    r.baselined,
                    r.fixed,
                    if matches!(fail_on, FailOn::None) {
                        "none"
                    } else {
                        r.fail_on.as_str()
                    }
                );
                if r.baseline_updated {
                    println!(
                        "baseline written: {}",
                        r.baseline_path.clone().unwrap_or_default()
                    );
                }
                for l in &r.limitations {
                    println!("note: {l}");
                }
                if !r.executed {
                    println!("note: nothing was executed (no --run).");
                }
            }
            Ok(r.verdict == "fail" && !matches!(fail_on, FailOn::None))
        }
        Cmd::Doctor { path } => {
            let det = plumbgraph_core::providers::detect(
                &path,
                &plumbgraph_core::providers::default_search_path(),
            );
            if json {
                print_json(&det)?;
            } else {
                for d in &det {
                    println!(
                        "{:<20} {:<16} {:<9} {:<11} {}",
                        d.id,
                        format!("{:?}", d.kind),
                        if d.available { "installed" } else { "missing" },
                        if d.relevant { "relevant" } else { "-" },
                        if d.available {
                            d.path.clone().unwrap_or_default()
                        } else if d.relevant {
                            format!("install: {}", d.install_hint)
                        } else {
                            String::new()
                        }
                    );
                }
                if let Some(w) = plumbgraph_core::providers::self_overwrite_warning() {
                    println!("\nwarning: {w}");
                }
                println!("
nothing was executed. `plumb enrich` runs SCIP indexers; `plumb verify --run` runs diagnostics tools and rule engines.");
            }
            Ok(false)
        }
        Cmd::Enrich {
            path,
            only,
            timeout_secs,
        } => {
            let search = plumbgraph_core::providers::default_search_path();
            let ids: Vec<String> = if only.is_empty() {
                plumbgraph_core::providers::detect(&path, &search)
                    .into_iter()
                    .filter(|d| {
                        d.kind == plumbgraph_core::providers::Kind::ScipIndexer
                            && d.available
                            && d.relevant
                    })
                    .map(|d| d.id)
                    .collect()
            } else {
                only
            };
            if ids.is_empty() {
                println!("no relevant SCIP indexer is installed (see `plumb doctor`); analysis stays tier-0");
                return Ok(false);
            }
            let runs = plumbgraph_core::providers::run_scip_indexers(
                &path,
                &ids,
                &search,
                std::time::Duration::from_secs(timeout_secs),
            );
            if json {
                print_json(&runs)?;
            } else {
                for r in &runs {
                    println!(
                        "{:<20} {:<8} {} ms {}{}",
                        r.id,
                        r.status,
                        r.duration_ms,
                        r.output.clone().unwrap_or_default(),
                        r.note
                            .as_ref()
                            .map(|n| format!("  ({n})"))
                            .unwrap_or_default()
                    );
                }
                println!("indexes in .plumbgraph/scip/ are picked up automatically by map/impact/dead-code/refs.");
            }
            Ok(runs
                .iter()
                .any(|r| r.status == "failed" || r.status == "timeout"))
        }
        Cmd::Init {
            path,
            agents,
            dry_run,
            no_gitignore,
        } => {
            let r = plumbgraph_core::init::init_with(&path, &agents, dry_run, !no_gitignore)?;
            if json {
                print_json(&r)?;
            } else {
                for w in &r.written {
                    println!("{} {w}", if dry_run { "would write" } else { "wrote" });
                }
                for u in &r.unchanged {
                    println!("unchanged {u}");
                }
                for n in &r.notes {
                    println!("note: {n}");
                }
                for s in &r.snippets {
                    println!("\n{s}");
                }
            }
            Ok(false)
        }
        Cmd::Mcp { root, allow_exec } => {
            let server =
                plumbgraph_mcp::Server::new(&root, cli.db.clone())?.with_allow_exec(allow_exec);
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            eprintln!("{}", plumbgraph_mcp::startup_line(&root, allow_exec));
            server.serve(stdin.lock(), stdout.lock())?;
            Ok(false)
        }
    }
}

pub fn main_entry() -> ExitCode {
    // Rust ignores SIGPIPE, so `plumb ... | head` makes println! panic with a backtrace hint.
    // A closed stdout is the reader's choice, not an error: exit quietly like other Unix tools.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("failed printing to std") && msg.contains("Broken pipe") {
            std::process::exit(141);
        }
        default_hook(info);
    }));
    let cli = Cli::parse();
    match run(cli) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}
