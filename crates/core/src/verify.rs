//! `plumb verify`: one pass/fail gate over dead code, dependency hallucination, test weakening,
//! diagnostics and rule engines, with baseline diffing so only **new** findings fail.
//!
//! Each finding gets a fingerprint that ignores line numbers (so moving code does not make an
//! old finding "new"): `sha256(category | rule | file | symbol-or-normalised-message)`.
//! Findings that come from a diff (`test-weakening`) are never baselined: they describe the
//! change under review, not the state of the tree.

use crate::deps::DepsReport;
use crate::ops::{self, DeadCodeParams, DepsParams, DiagInput, DiagParams, ScipOpts, Target};
use crate::{Finding, Level};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub const BASELINE_FILE: &str = "plumb-baseline.json";

#[derive(Debug, Clone)]
pub struct VerifyParams {
    /// Diff base for test-weakening (working tree vs this revision).
    pub base: String,
    /// Execute external tools: diagnostics tools (cargo, tsc, ruff, pyright) and rule engines.
    pub run: bool,
    /// Allow registry lookups (sends package names to PyPI/npm/crates.io).
    pub online_deps: bool,
    /// Saved tool output to merge in (executes nothing).
    pub diag_inputs: Vec<DiagInput>,
    pub baseline: Option<PathBuf>,
    pub update_baseline: bool,
    pub fail_on: Level,
    pub semgrep_config: Option<String>,
    pub timeout: Duration,
    pub scip: ScipOpts,
    pub library_mode: bool,
}

impl Default for VerifyParams {
    fn default() -> Self {
        VerifyParams {
            base: "HEAD".into(),
            run: false,
            online_deps: false,
            diag_inputs: vec![],
            baseline: None,
            update_baseline: false,
            fail_on: Level::Medium,
            semgrep_config: None,
            timeout: Duration::from_secs(120),
            scip: ScipOpts::default(),
            library_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub name: String,
    /// `ran` | `skipped` | `failed`
    pub status: String,
    pub findings: usize,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    /// `pass` | `fail`
    pub verdict: String,
    pub fail_on: String,
    pub new: Vec<Finding>,
    /// findings that are in the baseline (not shown)
    pub baselined: usize,
    /// baseline entries that no longer occur
    pub fixed: usize,
    pub total_findings: usize,
    pub steps: Vec<Step>,
    pub baseline_path: Option<String>,
    pub baseline_updated: bool,
    pub executed: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Baseline {
    version: u32,
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Entry {
    count: usize,
    category: String,
    rule: String,
    file: String,
    what: String,
}

fn normalise(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut in_digits = false;
    for c in msg.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

pub fn fingerprint(f: &Finding) -> String {
    let what = f.symbol.clone().unwrap_or_else(|| normalise(&f.message));
    let mut h = Sha256::new();
    for part in [&f.category, &f.rule, &f.file, &what] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    let d = h.finalize();
    {
        use std::fmt::Write as _;
        d.iter().take(10).fold(String::new(), |mut a, b| {
            let _ = write!(a, "{b:02x}");
            a
        })
    }
}

fn what_of(f: &Finding) -> String {
    f.symbol.clone().unwrap_or_else(|| normalise(&f.message))
}

pub fn run_verify(t: &Target, p: &VerifyParams) -> Result<VerifyReport> {
    let root = t
        .root
        .canonicalize()
        .with_context(|| format!("project root {}", t.root.display()))?;
    let mut steps = vec![];
    let mut all: Vec<Finding> = vec![];
    let mut diff_based: Vec<Finding> = vec![];
    let mut executed = false;

    // dead code
    match ops::run_dead_code(
        t,
        &DeadCodeParams {
            library_mode: p.library_mode,
            scip: p.scip.clone(),
            ..Default::default()
        },
    ) {
        Ok(r) => {
            steps.push(step("dead-code", "ran", r.findings.len(), None));
            all.extend(r.findings);
        }
        Err(e) => steps.push(step("dead-code", "failed", 0, Some(format!("{e:#}")))),
    }

    // dependencies
    let dp = DepsParams {
        offline: !p.online_deps,
        ..Default::default()
    };
    match ops::run_check_deps(t, &dp, None) {
        Ok(DepsReport {
            findings, offline, ..
        }) => {
            let note = offline.then(|| "offline: manifests and local resolution only; pass --online to check registries".to_string());
            steps.push(step("check-deps", "ran", findings.len(), note));
            all.extend(findings);
        }
        Err(e) => steps.push(step("check-deps", "failed", 0, Some(format!("{e:#}")))),
    }

    // test weakening (diff based)
    match ops::run_weakening(&root, &p.base, None) {
        Ok(r) => {
            steps.push(step(
                "test-weakening",
                "ran",
                r.findings.len(),
                Some(format!("vs {}", r.base)),
            ));
            diff_based.extend(r.findings);
        }
        Err(e) => steps.push(step("test-weakening", "skipped", 0, Some(format!("{e:#}")))),
    }

    // diagnostics: saved output, and/or tools when opted in
    if p.diag_inputs.is_empty() && !p.run {
        steps.push(step(
            "diagnostics",
            "skipped",
            0,
            Some("no saved output (--from) and --run not given; nothing was executed".into()),
        ));
    } else {
        let dparams = DiagParams {
            inputs: p.diag_inputs.clone(),
            run: p.run,
            timeout: p.timeout,
            ..Default::default()
        };
        match ops::run_diagnostics(&root, &dparams) {
            Ok(r) => {
                executed |= r.executed;
                let note = r
                    .tools
                    .iter()
                    .filter(|x| x.status != "ran" && x.status != "ingested")
                    .map(|x| format!("{}: {}", x.tool, x.status))
                    .collect::<Vec<_>>()
                    .join(", ");
                steps.push(step(
                    "diagnostics",
                    "ran",
                    r.findings.len(),
                    (!note.is_empty()).then_some(note),
                ));
                all.extend(r.findings);
            }
            Err(e) => steps.push(step("diagnostics", "failed", 0, Some(format!("{e:#}")))),
        }
    }

    // rule engines (semgrep / ast-grep): opt-in only
    if p.run {
        let (f, runs) =
            crate::rules::run_rule_engines(&root, p.semgrep_config.as_deref(), p.timeout);
        executed |= runs
            .iter()
            .any(|r| r.status == "ran" || r.status == "timeout");
        for r in runs {
            steps.push(step(&r.tool, &r.status, r.findings, r.note));
        }
        all.extend(f);
    } else {
        steps.push(step(
            "rule-engines",
            "skipped",
            0,
            Some("semgrep / ast-grep run only with --run".into()),
        ));
    }

    // baseline
    let bpath = p
        .baseline
        .clone()
        .unwrap_or_else(|| root.join(BASELINE_FILE));
    let baseline: Baseline = match std::fs::read_to_string(&bpath) {
        Ok(s) => serde_json::from_str(&s)
            .with_context(|| format!("baseline {} is not valid", bpath.display()))?,
        Err(_) => Baseline::default(),
    };
    let have_baseline = bpath.is_file();
    let mut current: BTreeMap<String, Entry> = BTreeMap::new();
    for f in &all {
        let e = current.entry(fingerprint(f)).or_insert_with(|| Entry {
            count: 0,
            category: f.category.clone(),
            rule: f.rule.clone(),
            file: f.file.clone(),
            what: what_of(f),
        });
        e.count += 1;
    }
    let mut remaining: BTreeMap<String, usize> = baseline
        .entries
        .iter()
        .map(|(k, v)| (k.clone(), v.count))
        .collect();
    let mut new: Vec<Finding> = vec![];
    let mut baselined = 0usize;
    for f in &all {
        let fp = fingerprint(f);
        match remaining.get_mut(&fp) {
            Some(n) if *n > 0 => {
                *n -= 1;
                baselined += 1;
            }
            _ => new.push(f.clone()),
        }
    }
    let fixed: usize = remaining.values().sum();
    let mut baseline_updated = false;
    if p.update_baseline {
        let b = Baseline {
            version: 1,
            entries: current,
        };
        std::fs::write(&bpath, serde_json::to_string_pretty(&b)? + "\n")
            .with_context(|| format!("writing {}", bpath.display()))?;
        baseline_updated = true;
    }
    new.extend(diff_based);
    new.sort_by(|a, b| {
        b.level
            .cmp(&a.level)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    let total = all.len();
    let tripped = new.iter().any(|f| f.level >= p.fail_on);
    let mut limitations = vec![
        "dead-code findings are name-based unless a SCIP index covers the file (see `plumb scip`); `new` means not in the baseline, not necessarily introduced by this change".into(),
        "baseline fingerprints ignore line numbers; renaming a symbol or file makes its findings new".into(),
    ];
    if !have_baseline {
        limitations.push(format!(
            "no baseline found at {}: every finding counts as new (create one with --update-baseline)",
            bpath.display()
        ));
    }
    Ok(VerifyReport {
        verdict: if tripped && !p.update_baseline {
            "fail"
        } else {
            "pass"
        }
        .into(),
        fail_on: p.fail_on.as_str().into(),
        new,
        baselined,
        fixed,
        total_findings: total,
        steps,
        baseline_path: Some(bpath.display().to_string()),
        baseline_updated,
        executed,
        limitations,
    })
}

fn step(name: &str, status: &str, findings: usize, note: Option<String>) -> Step {
    Step {
        name: name.into(),
        status: status.into(),
        findings,
        note: note.map(|n| crate::sanitize(&n)),
    }
}
