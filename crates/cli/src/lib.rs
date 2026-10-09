//! The `plumb` command line.

use anyhow::Result;
use clap::{Parser, Subcommand};
use plumbgraph_core::ops::{self, DeadCodeParams, DepsParams, Target};
use plumbgraph_core::{Finding, Level};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "plumb",
    version,
    about = "Know your code. Verify the change.",
    long_about = "Plumbgraph: tier-0 (tree-sitter) code intelligence and pre-submit checks for AI coding agents.\nPre-alpha: results are heuristic; every finding carries a source and a confidence."
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
        /// Only report files under this project-relative prefix
        #[arg(long)]
        only: Option<String>,
        /// Exit with status 1 when a finding of at least this level exists
        #[arg(long, value_enum, default_value = "none")]
        fail_on: FailOn,
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
    },
    /// Serve the tools over MCP (stdio, JSON-RPC)
    Mcp {
        /// Project root the tools are confined to (default: current directory)
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn render(f: &Finding) {
    println!(
        "{:<6} {:.2}  {}:{}  {}  [{}]",
        f.level.as_str().to_uppercase(),
        f.confidence,
        f.file,
        f.line,
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
            only,
            fail_on,
        } => {
            let p = DeadCodeParams {
                library_mode: lib,
                min_confidence: if include_low { 0.0 } else { min_confidence },
                kinds: if kinds.is_empty() { None } else { Some(kinds) },
                allow_file,
                path_prefix: only,
                include_test_only: !no_test_only,
            };
            let r = ops::run_dead_code(&target(path), &p)?;
            if json {
                print_json(&r)?;
            } else {
                for f in &r.findings {
                    render(f);
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
                println!("note: tier-0 name-based analysis; verify medium/low findings before deleting anything.");
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
        } => {
            let p = DepsParams {
                offline,
                path_prefix: only,
                min_confidence,
                check_declared: !no_check_declared,
            };
            let r = ops::run_check_deps(&target(path), &p, None)?;
            if json {
                print_json(&r)?;
            } else {
                for f in &r.findings {
                    render(f);
                }
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
        } => {
            let hits = ops::references(&target(path), &symbol, min_confidence, limit)?;
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
        Cmd::Mcp { root } => {
            let server = plumbgraph_mcp::Server::new(&root, cli.db.clone())?;
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            eprintln!("plumbgraph MCP server (stdio) root={}", root.display());
            server.serve(stdin.lock(), stdout.lock())?;
            Ok(false)
        }
    }
}

pub fn main_entry() -> ExitCode {
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
