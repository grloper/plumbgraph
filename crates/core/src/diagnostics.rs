//! Diagnostics: ingest or run real compiler / type-checker / linter output and normalise it
//! into [`Finding`]s (`category = "diagnostic"`) that carry `source` and `confidence`.
//!
//! Supported inputs: `cargo check|clippy --message-format=json`, `ruff check --output-format json`,
//! `pyright --outputjson`, `tsc --pretty false` text, and (module [`crate::lsp`]) LSP
//! `publishDiagnostics`.
//!
//! Ingesting a saved output file executes nothing. Running tools ([`run_tools`]) executes the
//! project's toolchain (`cargo check` runs build scripts and proc-macros) and is therefore
//! always an explicit opt-in at the call sites (CLI `--run`, MCP `allow_exec`).
//!
//! Confidence is a fixed per-source constant, *not* a calibrated probability: a compiler or
//! type-checker error is by construction a real tool-reported problem (0.98), but it can still
//! be an environment problem (missing dependency, wrong interpreter), which is why it is not 1.0.

use crate::{sanitize, Finding, Severity};
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `cargo check|clippy --message-format=json` (JSON lines)
    CargoJson,
    /// `ruff check --output-format json`
    Ruff,
    /// `pyright --outputjson`
    Pyright,
    /// `tsc --noEmit --pretty false` (plain text; tsc has no JSON output)
    Tsc,
}

impl std::str::FromStr for Format {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "cargo-json" | "clippy-json" => Format::CargoJson,
            "ruff-json" | "ruff" => Format::Ruff,
            "pyright-json" | "pyright" => Format::Pyright,
            "tsc" | "tsc-text" => Format::Tsc,
            other => bail!(
                "unknown diagnostics format `{}` (expected cargo-json, ruff-json, pyright-json, tsc)",
                sanitize(other)
            ),
        })
    }
}

impl Format {
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::CargoJson => "cargo-json",
            Format::Ruff => "ruff-json",
            Format::Pyright => "pyright-json",
            Format::Tsc => "tsc",
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ToolRun {
    pub tool: String,
    pub command: String,
    /// `ran` | `skipped` (not installed / not applicable) | `failed` | `timeout`
    pub status: String,
    pub findings: usize,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DiagReport {
    pub findings: Vec<Finding>,
    pub tools: Vec<ToolRun>,
    /// diagnostics whose file is outside the project root (dropped)
    pub dropped_outside_root: usize,
    /// true when at least one external process was started
    pub executed: bool,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub format: Format,
    pub program: String,
    pub args: Vec<String>,
}

/// Confidence constants per origin (see module docs).
pub(crate) fn confidence_for(origin: &str, sev: Severity) -> f64 {
    let hard = match origin {
        "compiler" | "typecheck" => 0.98,
        "lsp" => 0.95,
        _ => 0.88, // lint
    };
    match sev {
        Severity::Error => hard,
        Severity::Warning => (hard - 0.05).max(0.0),
        Severity::Info => 0.75,
    }
}

/// Lexically normalise `p` to a root-relative `/` path. `None` when it escapes the root.
pub(crate) fn rel_path(root: &Path, p: &str) -> Option<String> {
    let path = Path::new(p);
    let rel: PathBuf = if path.is_absolute() {
        path.strip_prefix(root).ok()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    let mut parts: Vec<String> = vec![];
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

pub(crate) fn clean_message(m: &str) -> String {
    let flat = m.replace(['\n', '\r', '\u{a0}'], " ");
    let one: Vec<&str> = flat.split_whitespace().collect();
    sanitize(&one.join(" "))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn make(
    origin: &str,
    source: &str,
    rule: &str,
    file: &str,
    line: u32,
    col: Option<u32>,
    sev: Severity,
    message: &str,
) -> Finding {
    let mut f = Finding::new(
        "diagnostic",
        &sanitize(rule),
        file,
        line.max(1),
        confidence_for(origin, sev),
        source,
        clean_message(message),
    );
    f.column = col;
    f.kind = origin.to_string();
    f.severity = sev;
    f.evidence = vec![format!(
        "reported by {} at {}:{}{}",
        source,
        file,
        line.max(1),
        col.map(|c| format!(":{c}")).unwrap_or_default()
    )];
    f.fp_risks = vec![
        "tool output depends on the local environment (toolchain version, installed dependencies, configuration)".into(),
    ];
    f
}

fn u32_of(v: &Value) -> u32 {
    v.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32
}

fn finish(mut v: Vec<Finding>) -> Vec<Finding> {
    let mut seen = HashSet::new();
    v.retain(|f| {
        seen.insert((
            f.file.clone(),
            f.line,
            f.column,
            f.rule.clone(),
            f.message.clone(),
        ))
    });
    v.sort_by(|a, b| {
        (&a.file, a.line, a.column, &a.rule).cmp(&(&b.file, b.line, b.column, &b.rule))
    });
    v
}

/// Parse tool output. Returns the findings and how many were dropped for pointing outside `root`.
pub fn parse(f: Format, text: &str, root: &Path) -> Result<(Vec<Finding>, usize)> {
    let mut out: Vec<Finding> = vec![];
    let mut dropped = 0usize;
    let mut push = |file: &str, mk: &dyn Fn(&str) -> Finding| match rel_path(root, file) {
        Some(r) => out.push(mk(&r)),
        None => dropped += 1,
    };
    match f {
        Format::CargoJson => {
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue; // build-script chatter and the like
                };
                if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
                    continue;
                }
                let m = &v["message"];
                let sev = match m["level"].as_str() {
                    Some("error") | Some("error: internal compiler error") => Severity::Error,
                    Some("warning") => Severity::Warning,
                    _ => continue,
                };
                let Some(span) = m["spans"]
                    .as_array()
                    .and_then(|a| a.iter().find(|s| s["is_primary"] == true))
                else {
                    continue;
                };
                let code = m["code"]["code"].as_str().unwrap_or("rustc");
                let (origin, source) = if code.starts_with("clippy::") {
                    ("lint", "lint:clippy")
                } else {
                    ("compiler", "compiler:rustc")
                };
                let file = span["file_name"].as_str().unwrap_or("");
                let (l, c) = (u32_of(&span["line_start"]), u32_of(&span["column_start"]));
                let msg = m["message"].as_str().unwrap_or("");
                push(file, &|r| {
                    make(origin, source, code, r, l, Some(c), sev, msg)
                });
            }
        }
        Format::Ruff => {
            let v: Value = serde_json::from_str(text).context("ruff output is not JSON")?;
            let arr = v
                .as_array()
                .ok_or_else(|| anyhow!("ruff JSON must be an array"))?;
            for d in arr {
                let sev = match d["severity"].as_str() {
                    Some("error") => Severity::Error,
                    Some("info") => Severity::Info,
                    _ => Severity::Warning,
                };
                let code = d["code"].as_str().unwrap_or("ruff");
                let file = d["filename"].as_str().unwrap_or("");
                let (l, c) = (
                    u32_of(&d["location"]["row"]),
                    u32_of(&d["location"]["column"]),
                );
                let msg = d["message"].as_str().unwrap_or("");
                push(file, &|r| {
                    make("lint", "lint:ruff", code, r, l, Some(c), sev, msg)
                });
            }
        }
        Format::Pyright => {
            let v: Value = serde_json::from_str(text).context("pyright output is not JSON")?;
            let arr = v["generalDiagnostics"]
                .as_array()
                .ok_or_else(|| anyhow!("pyright JSON has no `generalDiagnostics` array"))?;
            for d in arr {
                let sev = match d["severity"].as_str() {
                    Some("error") => Severity::Error,
                    Some("warning") => Severity::Warning,
                    _ => Severity::Info,
                };
                let rule = d["rule"].as_str().unwrap_or("pyright");
                let file = d["file"].as_str().unwrap_or("");
                // pyright ranges are 0-based
                let l = u32_of(&d["range"]["start"]["line"]) + 1;
                let c = u32_of(&d["range"]["start"]["character"]) + 1;
                let msg = d["message"].as_str().unwrap_or("");
                push(file, &|r| {
                    make(
                        "typecheck",
                        "typecheck:pyright",
                        rule,
                        r,
                        l,
                        Some(c),
                        sev,
                        msg,
                    )
                });
            }
        }
        Format::Tsc => {
            let re = Regex::new(r"^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$")
                .expect("static regex");
            for line in text.lines() {
                let Some(c) = re.captures(line.trim_end()) else {
                    continue;
                };
                let sev = if &c[4] == "error" {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                let (l, col) = (c[2].parse().unwrap_or(1), c[3].parse().unwrap_or(1));
                let (rule, msg) = (c[5].to_string(), c[6].to_string());
                push(&c[1], &|r| {
                    make(
                        "typecheck",
                        "typecheck:tsc",
                        &rule,
                        r,
                        l,
                        Some(col),
                        sev,
                        &msg,
                    )
                });
            }
        }
    }
    Ok((finish(out), dropped))
}

/// Tools that apply to the project, detected from its files. None of them is started here.
pub fn builtin_tools(root: &Path) -> Vec<ToolSpec> {
    let mut v = vec![];
    let has = |n: &str| root.join(n).exists();
    if has("Cargo.toml") {
        for (name, sub) in [("cargo-check", "check"), ("clippy", "clippy")] {
            v.push(ToolSpec {
                name: name.into(),
                format: Format::CargoJson,
                program: "cargo".into(),
                args: [sub, "--workspace", "--all-targets", "--message-format=json"]
                    .map(String::from)
                    .to_vec(),
            });
        }
    }
    if has("tsconfig.json") {
        let local = root.join("node_modules/.bin/tsc");
        v.push(ToolSpec {
            name: "tsc".into(),
            format: Format::Tsc,
            program: if local.is_file() {
                local.display().to_string()
            } else {
                "tsc".into()
            },
            args: ["--noEmit", "--pretty", "false", "-p", "."]
                .map(String::from)
                .to_vec(),
        });
    }
    if [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
    ]
    .iter()
    .any(|n| has(n))
    {
        v.push(ToolSpec {
            name: "ruff".into(),
            format: Format::Ruff,
            program: "ruff".into(),
            args: ["check", "--output-format", "json", "."]
                .map(String::from)
                .to_vec(),
        });
        v.push(ToolSpec {
            name: "pyright".into(),
            format: Format::Pyright,
            program: "pyright".into(),
            args: vec!["--outputjson".into()],
        });
    }
    v
}

const MAX_OUTPUT: u64 = 64 * 1024 * 1024;

pub(crate) enum Exec {
    Done {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    NotFound,
    Timeout,
    Failed(String),
}

pub(crate) fn exec(program: &str, args: &[String], cwd: &Path, timeout: Duration) -> Exec {
    let mut child = match Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Exec::NotFound,
        Err(e) => return Exec::Failed(e.to_string()),
    };
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let t_out = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = (&mut so).take(MAX_OUTPUT).read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = (&mut se).take(1 << 20).read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Exec::Failed(e.to_string()),
        }
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    match status {
        Some(s) => Exec::Done {
            code: s.code(),
            stdout,
            stderr,
        },
        None => Exec::Timeout,
    }
}

/// Run the given tools in `root` and normalise their output. **Executes the project's toolchain.**
pub fn run_tools(root: &Path, specs: &[ToolSpec], timeout: Duration) -> DiagReport {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut rep = DiagReport::default();
    let mut all = vec![];
    for s in specs {
        let cmd = format!("{} {}", s.program, s.args.join(" "));
        let mut run = ToolRun {
            tool: s.name.clone(),
            command: sanitize(&cmd),
            ..Default::default()
        };
        match exec(&s.program, &s.args, &root, timeout) {
            Exec::NotFound => {
                run.status = "skipped".into();
                run.note = Some(format!(
                    "`{}` is not installed or not on PATH",
                    sanitize(&s.program)
                ));
            }
            Exec::Timeout => {
                rep.executed = true;
                run.status = "timeout".into();
                run.note = Some(format!("killed after {} ms", timeout.as_millis()));
            }
            Exec::Failed(e) => {
                run.status = "failed".into();
                run.note = Some(sanitize(&e));
            }
            Exec::Done {
                code,
                stdout,
                stderr,
            } => {
                rep.executed = true;
                match parse(s.format, &stdout, &root) {
                    Ok((f, d))
                        if !(f.is_empty() && stdout.trim().is_empty() && code != Some(0)) =>
                    {
                        run.status = "ran".into();
                        run.findings = f.len();
                        rep.dropped_outside_root += d;
                        all.extend(f);
                    }
                    Ok(_) => {
                        run.status = "failed".into();
                        run.note = Some(format!(
                            "exit {:?} with no output: {}",
                            code,
                            sanitize(&stderr)
                        ));
                    }
                    Err(e) => {
                        run.status = "failed".into();
                        run.note = Some(format!(
                            "{}; stderr: {}",
                            sanitize(&format!("{e:#}")),
                            sanitize(&stderr)
                        ));
                    }
                }
            }
        }
        rep.tools.push(run);
    }
    rep.findings = finish(all);
    rep
}

/// Merge several reports (ingested files, tool runs, LSP) into one.
pub fn merge(reports: Vec<DiagReport>) -> DiagReport {
    let mut out = DiagReport::default();
    let mut all = vec![];
    for r in reports {
        all.extend(r.findings);
        out.tools.extend(r.tools);
        out.dropped_outside_root += r.dropped_outside_root;
        out.executed |= r.executed;
    }
    out.findings = finish(all);
    out
}

/// Keep findings at or above `min`.
pub fn filter_severity(rep: &mut DiagReport, min: Severity) {
    rep.findings.retain(|f| f.severity >= min);
}
