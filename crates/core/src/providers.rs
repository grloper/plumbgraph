//! Pluggable external providers: detection (never executes anything) and opt-in runs.
//!
//! A provider is an installed tool that adds facts to the graph or findings to a report:
//! SCIP indexers (compiler-grade references), diagnostics tools (compilers, linters, type
//! checkers) and rule engines (semgrep, ast-grep). `detect` only looks for executables;
//! `run_scip_indexers` starts an indexer **only when asked**, and only a catalog entry (never
//! a user-supplied command line).

use crate::diagnostics::{exec, Exec};
use crate::sanitize;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    ScipIndexer,
    Diagnostics,
    RuleEngine,
    LanguageServer,
}

struct Entry {
    id: &'static str,
    kind: Kind,
    /// file extensions (without dot) that make the provider relevant
    exts: &'static [&'static str],
    program: &'static str,
    /// SCIP indexers: argument template; `{out}` is replaced by the output path
    args: &'static [&'static str],
    runs_project_code: bool,
    install: &'static str,
}

const CATALOG: &[Entry] = &[
    Entry {
        id: "rust-analyzer-scip",
        kind: Kind::ScipIndexer,
        exts: &["rs"],
        program: "rust-analyzer",
        args: &["scip", ".", "--output", "{out}"],
        runs_project_code: true,
        install: "rustup component add rust-analyzer",
    },
    Entry {
        id: "scip-typescript",
        kind: Kind::ScipIndexer,
        exts: &["ts", "tsx", "js", "jsx"],
        program: "scip-typescript",
        args: &["index", "--infer-tsconfig", "--output", "{out}"],
        runs_project_code: true,
        install: "npm i -g @sourcegraph/scip-typescript",
    },
    Entry {
        id: "scip-python",
        kind: Kind::ScipIndexer,
        exts: &["py"],
        program: "scip-python",
        args: &[
            "index",
            ".",
            "--project-name",
            "project",
            "--output",
            "{out}",
        ],
        runs_project_code: true,
        install: "npm i -g @sourcegraph/scip-python",
    },
    Entry {
        id: "scip-go",
        kind: Kind::ScipIndexer,
        exts: &["go"],
        program: "scip-go",
        args: &["--output", "{out}"],
        runs_project_code: true,
        install: "go install github.com/scip-code/scip-go/cmd/scip-go@latest",
    },
    Entry {
        id: "scip-java",
        kind: Kind::ScipIndexer,
        exts: &["java"],
        program: "scip-java",
        args: &["index", "--output", "{out}"],
        runs_project_code: true,
        install: "see https://github.com/scip-code/scip-java (runs your Maven/Gradle build)",
    },
    Entry {
        id: "scip-dotnet",
        kind: Kind::ScipIndexer,
        exts: &["cs"],
        program: "scip-dotnet",
        args: &["index", "--output", "{out}"],
        runs_project_code: true,
        install: "dotnet tool install --global scip-dotnet",
    },
    Entry {
        id: "cargo-check",
        kind: Kind::Diagnostics,
        exts: &["rs"],
        program: "cargo",
        args: &[],
        runs_project_code: true,
        install: "rustup",
    },
    Entry {
        id: "clippy",
        kind: Kind::Diagnostics,
        exts: &["rs"],
        program: "cargo-clippy",
        args: &[],
        runs_project_code: true,
        install: "rustup component add clippy",
    },
    Entry {
        id: "tsc",
        kind: Kind::Diagnostics,
        exts: &["ts", "tsx"],
        program: "tsc",
        args: &[],
        runs_project_code: false,
        install: "npm i -D typescript",
    },
    Entry {
        id: "ruff",
        kind: Kind::Diagnostics,
        exts: &["py"],
        program: "ruff",
        args: &[],
        runs_project_code: false,
        install: "uv tool install ruff",
    },
    Entry {
        id: "pyright",
        kind: Kind::Diagnostics,
        exts: &["py"],
        program: "pyright",
        args: &[],
        runs_project_code: false,
        install: "uv tool install pyright",
    },
    Entry {
        id: "semgrep",
        kind: Kind::RuleEngine,
        exts: &["py", "js", "ts", "tsx", "go", "java", "cs", "rs"],
        program: "semgrep",
        args: &[],
        runs_project_code: false,
        install: "uv tool install semgrep (LGPL-2.1; invoked as a subprocess only)",
    },
    Entry {
        id: "ast-grep",
        kind: Kind::RuleEngine,
        exts: &["py", "js", "ts", "tsx", "go", "java", "cs", "rs"],
        program: "ast-grep",
        args: &[],
        runs_project_code: false,
        install: "npm i -g @ast-grep/cli",
    },
    Entry {
        id: "gopls",
        kind: Kind::LanguageServer,
        exts: &["go"],
        program: "gopls",
        args: &[],
        runs_project_code: true,
        install: "go install golang.org/x/tools/gopls@latest",
    },
    Entry {
        id: "pyright-langserver",
        kind: Kind::LanguageServer,
        exts: &["py"],
        program: "pyright-langserver",
        args: &["--stdio"],
        runs_project_code: false,
        install: "uv tool install pyright",
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct Detected {
    pub id: String,
    pub kind: Kind,
    pub program: String,
    pub available: bool,
    pub path: Option<String>,
    /// the project contains files this provider understands
    pub relevant: bool,
    /// running it may execute project code (build scripts, proc-macros, package managers)
    pub runs_project_code: bool,
    pub install_hint: String,
}

/// Directories to search: `$PATH`.
pub fn default_search_path() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// Windows `PATHEXT` default, used when the variable is unset or yields nothing usable.
pub const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Extensions `std::process::Command` can start on Windows (`.bat`/`.cmd` go through
/// `cmd.exe`). Other `PATHEXT` entries (`.js`, `.vbs`, `.msc`, ...) need a host program,
/// so a provider found only under such a name could not be run and is not reported.
const RUNNABLE_EXTS: &[&str] = &[".com", ".exe", ".bat", ".cmd"];

/// Parse a `PATHEXT`-style list (`.COM;.EXE;...`) into lower-case, dot-prefixed,
/// de-duplicated extensions in their original order, keeping only `RUNNABLE_EXTS`.
pub fn parse_pathext(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut e = part.to_ascii_lowercase();
        if !e.starts_with('.') {
            e.insert(0, '.');
        }
        if RUNNABLE_EXTS.contains(&e.as_str()) && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Executable extensions to try after a bare program name: from `PATHEXT` on Windows,
/// none elsewhere (Unix looks up the exact name and checks the execute bit).
pub fn executable_extensions() -> Vec<String> {
    #[cfg(windows)]
    {
        let raw = std::env::var("PATHEXT").unwrap_or_default();
        let exts = parse_pathext(&raw);
        if exts.is_empty() {
            parse_pathext(DEFAULT_PATHEXT)
        } else {
            exts
        }
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// File names to try for `program`: the exact name when there is no extension list or the
/// name already ends in a listed extension; otherwise `program` + each extension, in order.
/// The bare name is deliberately not tried when extensions apply: npm puts an
/// extensionless sh shim next to the `.cmd` one, and Windows cannot run the sh shim.
fn candidate_names(program: &str, exe_exts: &[String]) -> Vec<String> {
    let lower = program.to_ascii_lowercase();
    if exe_exts.is_empty() || exe_exts.iter().any(|e| lower.ends_with(e.as_str())) {
        return vec![program.to_string()];
    }
    exe_exts.iter().map(|e| format!("{program}{e}")).collect()
}

fn find_program(
    program: &str,
    search: &[PathBuf],
    root: &Path,
    exe_exts: &[String],
) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = search.to_vec();
    // project-local JS tooling (tsc, scip-typescript, ast-grep) installed by npm
    dirs.push(root.join("node_modules/.bin"));
    let names = candidate_names(program, exe_exts);
    // directory order first, then extension order: the same precedence as cmd.exe
    for d in dirs {
        for name in &names {
            let p = d.join(name);
            if !p.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if p.metadata()
                    .map(|m| m.permissions().mode() & 0o111 == 0)
                    .unwrap_or(true)
                {
                    continue;
                }
            }
            return Some(p);
        }
    }
    None
}

fn project_exts(root: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let skip = [
        "node_modules",
        "target",
        "vendor",
        "dist",
        "build",
        ".git",
        ".plumbgraph",
        "venv",
        ".venv",
    ];
    let mut n = 0usize;
    for e in ignore::WalkBuilder::new(root)
        .filter_entry(move |e| !skip.iter().any(|s| e.file_name() == *s))
        .build()
        .flatten()
    {
        if let Some(x) = e.path().extension().and_then(|x| x.to_str()) {
            out.insert(x.to_string());
        }
        n += 1;
        if n > 20_000 {
            break;
        }
    }
    out
}

/// Which providers are installed and relevant. Looks for executables only; runs nothing.
/// On Windows a bare name such as `cargo` is matched as `cargo.exe`, `cargo.cmd`, ...
/// following `PATHEXT` (see `executable_extensions`).
pub fn detect(root: &Path, search: &[PathBuf]) -> Vec<Detected> {
    detect_with_exts(root, search, &executable_extensions())
}

/// `detect` with an explicit executable-extension list (`[]` = exact names, the Unix rule).
/// Lets the Windows lookup rules be tested on any platform.
pub fn detect_with_exts(root: &Path, search: &[PathBuf], exe_exts: &[String]) -> Vec<Detected> {
    let exts = project_exts(root);
    CATALOG
        .iter()
        .map(|e| {
            let found = find_program(e.program, search, root, exe_exts);
            Detected {
                id: e.id.into(),
                kind: e.kind,
                program: e.program.into(),
                available: found.is_some(),
                path: found.map(|p| p.display().to_string()),
                relevant: e.exts.iter().any(|x| exts.contains(*x)),
                runs_project_code: e.runs_project_code,
                install_hint: e.install.into(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProviderRun {
    pub id: String,
    /// `ran` | `skipped` (not installed) | `failed` | `timeout` | `refused` (not a catalog SCIP indexer)
    pub status: String,
    pub output: Option<String>,
    pub duration_ms: u128,
    pub note: Option<String>,
}

/// Run the named SCIP indexers (catalog ids only) inside `root`, writing
/// `<root>/.plumbgraph/scip/<id>.scip`. **Executes the indexer, which may run project code.**
pub fn run_scip_indexers(
    root: &Path,
    ids: &[String],
    search: &[PathBuf],
    timeout: Duration,
) -> Vec<ProviderRun> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let out_dir = root.join(".plumbgraph").join("scip");
    let mut runs = vec![];
    for id in ids {
        let mut run = ProviderRun {
            id: sanitize(id),
            ..Default::default()
        };
        let Some(e) = CATALOG
            .iter()
            .find(|e| e.id == id && e.kind == Kind::ScipIndexer)
        else {
            run.status = "refused".into();
            run.note = Some("not a known SCIP indexer; only catalog entries can be run".into());
            runs.push(run);
            continue;
        };
        let Some(prog) = find_program(e.program, search, &root, &executable_extensions()) else {
            run.status = "skipped".into();
            run.note = Some(format!("`{}` not found; install: {}", e.program, e.install));
            runs.push(run);
            continue;
        };
        if let Err(err) = std::fs::create_dir_all(&out_dir) {
            run.status = "failed".into();
            run.note = Some(sanitize(&err.to_string()));
            runs.push(run);
            continue;
        }
        let out = out_dir.join(format!("{}.scip", e.id));
        let _ = std::fs::remove_file(&out);
        let args: Vec<String> = e
            .args
            .iter()
            .map(|a| {
                if *a == "{out}" {
                    out.display().to_string()
                } else if *a == "project" {
                    root.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "project".into())
                } else {
                    a.to_string()
                }
            })
            .collect();
        let start = Instant::now();
        match exec(&prog.display().to_string(), &args, &root, timeout) {
            Exec::Done { code, .. } if code == Some(0) && out.is_file() => {
                run.status = "ran".into();
                run.output = Some(out.display().to_string());
            }
            Exec::Done { code, stderr, .. } => {
                run.status = "failed".into();
                run.note = Some(format!("exit {:?}: {}", code, sanitize(&stderr)));
            }
            Exec::Timeout => {
                run.status = "timeout".into();
                run.note = Some(format!("killed after {} s", timeout.as_secs()));
            }
            Exec::NotFound => {
                run.status = "skipped".into();
            }
            Exec::Failed(m) => {
                run.status = "failed".into();
                run.note = Some(sanitize(&m));
            }
        }
        run.duration_ms = start.elapsed().as_millis();
        runs.push(run);
    }
    runs
}

/// Windows keeps a running `.exe` open, so `cargo install` fails with "Access is denied" while an
/// MCP host still runs `plumb mcp`. `probe` tries to open `exe` for writing; this turns a failure
/// into the advice shown by `plumb doctor`. Platform independent so it can be tested anywhere.
pub fn overwrite_warning(
    exe: &Path,
    probe: impl Fn(&Path) -> std::io::Result<()>,
) -> Option<String> {
    let e = probe(exe).err()?;
    let denied = e.kind() == std::io::ErrorKind::PermissionDenied;
    Some(format!(
        "cannot overwrite the running executable {} ({}{}). `cargo install` will fail while it is in use: stop `plumb mcp` / close the MCP hosts (Claude, Cursor, ...) that started it first, or install elsewhere with `cargo install --path crates/cli --locked --root <dir>` and point the host at that binary",
        exe.display(),
        e,
        if denied { ": another process, likely a running `plumb mcp`, holds it open" } else { "" }
    ))
}

/// `plumb doctor` check for the running binary. Windows only: elsewhere a running binary can be
/// replaced (the old file is unlinked), so there is nothing to warn about.
pub fn self_overwrite_warning() -> Option<String> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().ok()?;
        overwrite_warning(&exe, |p| {
            std::fs::OpenOptions::new().write(true).open(p).map(|_| ())
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}
