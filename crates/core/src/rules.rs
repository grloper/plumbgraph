//! Rule engines as providers: semgrep (`semgrep scan --json`) and ast-grep
//! (`ast-grep scan --json=stream`). Output is normalised into [`Finding`]s
//! (`category = "rule"`, `source = "semgrep" | "ast-grep"`).
//!
//! Both are invoked as subprocesses (semgrep is LGPL-2.1: never linked). They parse code but do
//! not execute it. Rule files come from the repository (or from the operator); rule *content*
//! is data, but remote rule packs (`p/...`) need the network, so those are used only when the
//! operator passes `--semgrep-config` explicitly.

use crate::diagnostics::{exec, rel_path, required_text, Exec};
use crate::providers::{default_search_path, detect, Kind};
use crate::{sanitize, Finding, Severity};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
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
    if let Some(errors) = v.get("errors") {
        let errors = errors
            .as_array()
            .ok_or_else(|| anyhow!("semgrep `errors` must be an array"))?;
        if !errors.is_empty() {
            return Err(anyhow!("semgrep reported {} analysis errors", errors.len()));
        }
    }
    let results = v["results"]
        .as_array()
        .ok_or_else(|| anyhow!("semgrep output has no `results` array"))?;
    let mut out = vec![];
    for r in results {
        let file = required_text(&r["path"], "semgrep path")?;
        let Some(path) = rel_path(root, file) else {
            continue;
        };
        let sev = match r["extra"]["severity"].as_str() {
            Some("ERROR") => Severity::Error,
            Some("WARNING") => Severity::Warning,
            _ => Severity::Info,
        };
        out.push(finding(
            "semgrep",
            required_text(&r["check_id"], "semgrep check_id")?,
            &path,
            r["start"]["line"].as_u64().unwrap_or(1) as u32,
            r["start"]["col"].as_u64().map(|c| c as u32),
            sev,
            required_text(&r["extra"]["message"], "semgrep message")?,
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
        let file = required_text(&v["file"], "ast-grep file")?;
        let Some(path) = rel_path(root, file) else {
            continue;
        };
        let sev = match v["severity"].as_str() {
            Some("error") => Severity::Error,
            Some("warning") => Severity::Warning,
            _ => Severity::Info,
        };
        out.push(finding(
            "ast-grep",
            required_text(&v["ruleId"], "ast-grep ruleId")?,
            &path,
            astgrep_position(&v["range"]["start"]["line"])?,
            Some(astgrep_position(&v["range"]["start"]["column"])?),
            sev,
            required_text(&v["message"], "ast-grep message")?,
        ));
    }
    Ok(out)
}

fn astgrep_position(value: &Value) -> Result<u32> {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .and_then(|v| v.checked_add(1))
        .ok_or_else(|| anyhow!("invalid ast-grep source position"))
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct EngineRun {
    pub tool: String,
    /// `ran` | `skipped` | `failed` | `timeout`
    pub status: String,
    pub findings: usize,
    /// True once the configured engine process was started, even if it failed.
    pub executed: bool,
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
    run_rule_engines_with_search(root, semgrep_config, timeout, &default_search_path())
}

fn run_rule_engines_with_search(
    root: &Path,
    semgrep_config: Option<&str>,
    timeout: Duration,
    search: &[PathBuf],
) -> (Vec<Finding>, Vec<EngineRun>) {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let det = detect(&root, search);
    let mut findings = vec![];
    let mut runs = vec![];
    for d in det.iter().filter(|d| d.kind == Kind::RuleEngine) {
        let mut run = EngineRun {
            tool: d.id.clone(),
            ..Default::default()
        };
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
        // An optional, unconfigured engine may be skipped; a configured one is
        // required even when its executable is absent.
        if !d.available {
            run.status = "failed".into();
            run.note = Some(format!(
                "configured engine not installed ({})",
                d.install_hint
            ));
            runs.push(run);
            continue;
        }
        let prog = d.path.clone().unwrap_or_else(|| d.program.clone());
        match exec(&prog, &args, &root, timeout) {
            Exec::Done {
                stdout,
                stderr,
                code,
            } => {
                run.executed = true;
                match parser(&stdout, &root) {
                    Ok(f) => {
                        let completed = code == Some(0) || (code == Some(1) && !f.is_empty());
                        run.status = if completed { "ran" } else { "failed" }.into();
                        if !completed {
                            run.note = Some(format!(
                                "exit {:?} without a completed rule check; stderr: {}",
                                code,
                                sanitize(&stderr)
                            ));
                        }
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
                }
            }
            Exec::Timeout => {
                run.executed = true;
                run.status = "timeout".into();
                run.note = Some(format!("killed after {} s", timeout.as_secs()));
            }
            Exec::NotFound => {
                run.status = "failed".into();
                run.note = Some("configured engine executable was not found".into());
            }
            Exec::Incomplete(m) => {
                run.executed = true;
                run.status = "failed".into();
                run.note = Some(sanitize(&m));
            }
            Exec::Failed(m) => {
                run.status = "failed".into();
                run.note = Some(sanitize(&m));
            }
        }
        runs.push(run);
    }
    (findings, runs)
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    fn run(root: &Path, config: Option<&str>) -> (Vec<Finding>, Vec<EngineRun>) {
        run_rule_engines_with_search(root, config, Duration::from_millis(100), &[])
    }

    #[test]
    fn missing_engines_fail_only_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), None)
            .1
            .iter()
            .all(|r| r.status == "skipped"));
        let explicit = run(dir.path(), Some("rules.yml"));
        assert_eq!(
            explicit
                .1
                .iter()
                .find(|r| r.tool == "semgrep")
                .unwrap()
                .status,
            "failed"
        );
        std::fs::write(dir.path().join(".semgrep.yml"), "rules: []").unwrap();
        std::fs::write(dir.path().join("sgconfig.yml"), "ruleDirs: []").unwrap();
        assert!(run(dir.path(), None).1.iter().all(|r| r.status == "failed"));
    }

    #[cfg(unix)]
    fn fake(root: &Path, tool: &str, output: &str, code: i32) {
        use std::os::unix::fs::PermissionsExt;
        let bin = root.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = bin.join(tool);
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s\\n' '{output}'\nexit {code}\n"),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn rule_process_exit_status_cannot_be_hidden_by_empty_or_invalid_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".semgrep.yml"), "rules: []").unwrap();
        std::fs::write(dir.path().join("sgconfig.yml"), "ruleDirs: []").unwrap();
        for code in [1, 2, 7] {
            fake(dir.path(), "semgrep", r#"{"results":[]}"#, code);
            fake(dir.path(), "ast-grep", "", code);
            assert!(run(dir.path(), None).1.iter().all(|r| r.status == "failed"));
        }
        fake(dir.path(), "semgrep", r#"{"results":[{}]}"#, 0);
        fake(dir.path(), "ast-grep", "{}", 0);
        assert!(run(dir.path(), None).1.iter().all(|r| r.status == "failed"));
        fake(dir.path(), "semgrep", r#"{"results":[],"errors":[]}"#, 0);
        fake(dir.path(), "ast-grep", "", 0);
        assert!(run(dir.path(), None).1.iter().all(|r| r.status == "ran"));
    }

    #[test]
    #[cfg(unix)]
    fn findings_exit_is_valid_but_error_exit_preserves_findings_and_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sgconfig.yml"), "ruleDirs: []").unwrap();
        let output = r#"{"file":"a.py","ruleId":"test","message":"finding","range":{"start":{"line":0,"column":0}},"severity":"error"}"#;
        for (code, status) in [(0, "ran"), (1, "ran"), (2, "failed")] {
            fake(dir.path(), "ast-grep", output, code);
            let (findings, runs) = run(dir.path(), None);
            assert!(runs.iter().find(|r| r.tool == "ast-grep").unwrap().executed);
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].source, "ast-grep");
            assert!(findings[0].confidence > 0.0);
            assert_eq!(
                runs.iter().find(|r| r.tool == "ast-grep").unwrap().status,
                status
            );
        }
    }

    #[test]
    fn semgrep_analysis_errors_are_not_an_empty_success() {
        assert!(parse_semgrep(
            r#"{"results":[],"errors":[{"message":"bad config"}]}"#,
            Path::new("/project")
        )
        .is_err());
        assert!(parse_semgrep(r#"{"results":[],"errors":{}}"#, Path::new("/project")).is_err());
    }
}
