//! Rule engines as providers: semgrep (`semgrep scan --json`) and ast-grep
//! (`ast-grep scan --json=stream`). Output is normalised into [`Finding`]s
//! (`category = "rule"`, `source = "semgrep" | "ast-grep"`).
//!
//! Both are invoked as subprocesses (semgrep is LGPL-2.1: never linked). They parse code but do
//! not execute it. Rule files come from the repository (or from the operator); rule *content*
//! is data, but remote rule packs (`p/...`) need the network, so those are used only when the
//! operator passes `--semgrep-config` explicitly.

use crate::diagnostics::{exec, rel_path, Exec};
use crate::providers::{default_search_path, detect, Kind};
use crate::{sanitize, Finding, Severity};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

type Parser = fn(&str, &Path) -> Result<Vec<Finding>>;

const CONF_ERROR_WARNING: f64 = 0.85;
const CONF_INFO: f64 = 0.7;

fn finding(
    source: &str,
    rule: &str,
    file: &str,
    line: u32,
    col: Option<u32>,
    sev: Severity,
    msg: &str,
) -> Finding {
    let conf = if sev == Severity::Info {
        CONF_INFO
    } else {
        CONF_ERROR_WARNING
    };
    let mut f = Finding::new(
        "rule",
        &sanitize(rule),
        file,
        line,
        conf,
        source,
        sanitize(msg),
    );
    f.column = col;
    f.severity = sev;
    f.kind = "rule-match".into();
    f.evidence = vec![format!("{source} rule `{}` matched", sanitize(rule))];
    f.fp_risks = vec![
        "a rule match is a pattern match, not proof of a defect; review the rule's intent".into(),
    ];
    f
}

pub fn parse_semgrep(json: &str, root: &Path) -> Result<Vec<Finding>> {
    let v: Value =
        serde_json::from_str(json).map_err(|e| anyhow!("semgrep output is not JSON: {e}"))?;
    let results = v["results"]
        .as_array()
        .ok_or_else(|| anyhow!("semgrep output has no `results` array"))?;
    let mut out = vec![];
    for r in results {
        let Some(path) = r["path"].as_str().and_then(|p| rel_path(root, p)) else {
            continue;
        };
        let sev = match r["extra"]["severity"].as_str() {
            Some("ERROR") => Severity::Error,
            Some("WARNING") => Severity::Warning,
            _ => Severity::Info,
        };
        out.push(finding(
            "semgrep",
            r["check_id"].as_str().unwrap_or("semgrep"),
            &path,
            r["start"]["line"].as_u64().unwrap_or(1) as u32,
            r["start"]["col"].as_u64().map(|c| c as u32),
            sev,
            r["extra"]["message"].as_str().unwrap_or(""),
        ));
    }
    Ok(out)
}

/// ast-grep's `--json=stream`: one JSON object per line, 0-based positions.
pub fn parse_astgrep(text: &str, root: &Path) -> Result<Vec<Finding>> {
    let mut out = vec![];
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(l)
            .map_err(|e| anyhow!("ast-grep output is not JSON lines: {e}"))?;
        let Some(path) = v["file"].as_str().and_then(|p| rel_path(root, p)) else {
            continue;
        };
        let sev = match v["severity"].as_str() {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            _ => Severity::Info,
        };
        out.push(finding(
            "ast-grep",
            v["ruleId"].as_str().unwrap_or("ast-grep"),
            &path,
            v["range"]["start"]["line"].as_u64().unwrap_or(0) as u32 + 1,
            v["range"]["start"]["column"].as_u64().map(|c| c as u32 + 1),
            sev,
            v["message"].as_str().unwrap_or(""),
        ));
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct EngineRun {
    pub tool: String,
    /// `ran` | `skipped` | `failed` | `timeout`
    pub status: String,
    pub findings: usize,
    pub note: Option<String>,
}

/// Run the rule engines that are installed **and** configured for this project.
/// semgrep: `semgrep_config` if given, else `.semgrep.yml` / `semgrep.yml` / `.semgrep/` in the root.
/// ast-grep: only with an `sgconfig.yml` in the root. **Starts external processes.**
pub fn run_rule_engines(
    root: &Path,
    semgrep_config: Option<&str>,
    timeout: Duration,
) -> (Vec<Finding>, Vec<EngineRun>) {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let det = detect(&root, &default_search_path());
    let mut findings = vec![];
    let mut runs = vec![];
    for d in det.iter().filter(|d| d.kind == Kind::RuleEngine) {
        let mut run = EngineRun {
            tool: d.id.clone(),
            ..Default::default()
        };
        if !d.available {
            run.status = "skipped".into();
            run.note = Some(format!("not installed ({})", d.install_hint));
            runs.push(run);
            continue;
        }
        let prog = d.path.clone().unwrap_or_else(|| d.program.clone());
        let (args, parser): (Vec<String>, Parser) = if d.id == "semgrep" {
            let cfg = semgrep_config.map(String::from).or_else(|| {
                [".semgrep.yml", "semgrep.yml", ".semgrep"]
                    .iter()
                    .find(|n| root.join(n).exists())
                    .map(|n| n.to_string())
            });
            let Some(cfg) = cfg else {
                run.status = "skipped".into();
                run.note = Some(
                    "no semgrep config (.semgrep.yml, semgrep.yml, .semgrep/ or --semgrep-config)"
                        .into(),
                );
                runs.push(run);
                continue;
            };
            (
                [
                    "scan",
                    "--json",
                    "--quiet",
                    "--metrics=off",
                    "--disable-version-check",
                    "--config",
                ]
                .iter()
                .map(|s| s.to_string())
                .chain([cfg, ".".to_string()])
                .collect(),
                parse_semgrep,
            )
        } else {
            if !root.join("sgconfig.yml").is_file() {
                run.status = "skipped".into();
                run.note = Some("no sgconfig.yml in the project root".into());
                runs.push(run);
                continue;
            }
            (
                ["scan", "--json=stream"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                parse_astgrep,
            )
        };
        match exec(&prog, &args, &root, timeout) {
            Exec::Done {
                stdout,
                stderr,
                code,
            } => match parser(&stdout, &root) {
                Ok(f) => {
                    run.status = "ran".into();
                    run.findings = f.len();
                    findings.extend(f);
                }
                Err(e) => {
                    run.status = "failed".into();
                    run.note = Some(format!(
                        "exit {:?}: {}; stderr: {}",
                        code,
                        sanitize(&format!("{e:#}")),
                        sanitize(&stderr)
                    ));
                }
            },
            Exec::Timeout => {
                run.status = "timeout".into();
                run.note = Some(format!("killed after {} s", timeout.as_secs()));
            }
            Exec::NotFound => run.status = "skipped".into(),
            Exec::Failed(m) => {
                run.status = "failed".into();
                run.note = Some(sanitize(&m));
            }
        }
        runs.push(run);
    }
    (findings, runs)
}
