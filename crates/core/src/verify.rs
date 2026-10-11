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
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const BASELINE_FILE: &str = "plumb-baseline.json";

/// Explicit per-check gate policy. Heuristic dead code is advisory unless opted in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerifyPolicy {
    pub enforce_dead_code: bool,
    pub dependencies: Level,
    pub weakening: Level,
    pub diagnostics: Level,
    pub rules: Level,
}

impl Default for VerifyPolicy {
    fn default() -> Self {
        Self {
            enforce_dead_code: false,
            dependencies: Level::Medium,
            weakening: Level::Medium,
            diagnostics: Level::Medium,
            rules: Level::Medium,
        }
    }
}

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
    /// Threshold only for explicitly enforced dead-code findings.
    pub fail_on: Level,
    pub policy: VerifyPolicy,
    /// Immutable Git commit containing the trusted baseline; also the weakening diff base.
    pub trusted_baseline: Option<String>,
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
            policy: VerifyPolicy::default(),
            trusted_baseline: None,
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
    pub schema_version: u32,
    /// `pass` | `fail` | `incomplete` | `baseline-updated`
    pub verdict: String,
    pub fail_on: String,
    pub policy: VerifyPolicy,
    pub trusted_baseline: Option<String>,
    pub index: Option<crate::index::IndexStats>,
    pub scip: Option<crate::scip::ScipStats>,
    /// Unobserved provider entries retained conservatively, not counted as fixed.
    pub baseline_unobserved: usize,
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
    // Refuse automatic cache/policy symlink escapes before any analyzer opens files.
    for rel in [
        ".plumbgraph",
        ".plumbgraph/index.db",
        ".plumbgraph/index.db-wal",
        ".plumbgraph/index.db-shm",
        ".plumbgraph/allow.toml",
    ] {
        let path = root.join(rel);
        if let Ok(md) = std::fs::symlink_metadata(&path) {
            ensure!(
                !md.file_type().is_symlink(),
                "project cache/policy symlinks are not supported: {}",
                rel
            );
        }
    }
    if let Some(db) = &t.db {
        let path = if db.is_absolute() {
            db.clone()
        } else {
            std::env::current_dir()?.join(db)
        };
        ensure!(
            path.starts_with(&root)
                && !path
                    .components()
                    .any(|c| c == std::path::Component::ParentDir),
            "verify database must stay inside project root"
        );
        let parent = path.parent().context("database path has no parent")?;
        let existing = parent
            .ancestors()
            .find(|p| p.exists())
            .context("database has no existing ancestor")?;
        ensure!(
            existing.canonicalize()?.starts_with(&root),
            "verify database must stay inside project root"
        );
        for suffix in ["", "-wal", "-shm"] {
            let candidate = PathBuf::from(format!("{}{suffix}", path.display()));
            if let Ok(md) = std::fs::symlink_metadata(candidate) {
                ensure!(
                    !md.file_type().is_symlink(),
                    "verify database symlinks are not supported"
                );
            }
        }
    }
    let mut steps = vec![];
    let mut all: Vec<Finding> = vec![];
    let mut diff_based: Vec<Finding> = vec![];
    let mut executed = false;
    let mut limitations = vec![
        "baseline fingerprints ignore line numbers; renaming a symbol or file makes its findings new".into(),
        "a pass covers the selected checks only; skipped optional checks provide no assurance".into(),
    ];
    let mut index = None;
    let mut scip = None;
    let (bpath, baseline, have_baseline) = read_baseline(&root, p)?;

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
            let incomplete = r.index.parse_errors > 0;
            steps.push(step(
                "dead-code",
                if incomplete { "failed" } else { "ran" },
                r.findings.len(),
                incomplete.then(|| format!("{} files could not be parsed", r.index.parse_errors)),
            ));
            if r.index.files_with_syntax_errors > 0 {
                limitations.push(format!(
                    "{} indexed files contain syntax errors; references may be incomplete",
                    r.index.files_with_syntax_errors
                ));
            }
            limitations.extend(r.limitations);
            limitations.extend(r.notes);
            index = Some(r.index);
            scip = r.scip;
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
            findings,
            offline,
            lookups,
            ..
        }) => {
            let note = offline.then(|| "offline: manifests and local resolution only; pass --online to check registries".to_string());
            let unknown = lookups
                .iter()
                .filter(|l| l.result.starts_with("unknown"))
                .count();
            steps.push(step(
                "check-deps",
                if !offline && unknown > 0 {
                    "failed"
                } else {
                    "ran"
                },
                findings.len(),
                if !offline && unknown > 0 {
                    Some(format!(
                        "{unknown} requested registry lookups were inconclusive"
                    ))
                } else {
                    note
                },
            ));
            all.extend(findings);
        }
        Err(e) => steps.push(step("check-deps", "failed", 0, Some(format!("{e:#}")))),
    }

    // test weakening (diff based)
    match ops::run_weakening(
        &root,
        p.trusted_baseline.as_deref().unwrap_or(&p.base),
        None,
    ) {
        Ok(r) => {
            steps.push(step(
                "test-weakening",
                "ran",
                r.findings.len(),
                Some(format!("vs {}", r.base)),
            ));
            diff_based.extend(r.findings);
        }
        Err(e) => steps.push(step("test-weakening", "failed", 0, Some(format!("{e:#}")))),
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
                let incomplete = r
                    .tools
                    .iter()
                    .any(|x| x.status != "ran" && x.status != "ingested");
                for tool in &r.tools {
                    steps.push(step(
                        &format!("diagnostics/{}", tool.tool),
                        &tool.status,
                        tool.findings,
                        tool.note.clone(),
                    ));
                }
                steps.push(step(
                    "diagnostics",
                    if incomplete { "failed" } else { "ran" },
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
        executed |= runs.iter().any(|r| r.executed);
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
    // Optional providers may not have run this time. Keep their baseline entries instead
    // of reporting them fixed or silently deleting them during static-only maintenance.
    let completed = |name: &str| steps.iter().any(|s| s.name == name && s.status == "ran");
    let observed = |e: &Entry| match e.category.as_str() {
        "dead" => completed("dead-code"),
        "deps" => completed("check-deps"),
        _ => false,
    };
    let fixed: usize = remaining
        .iter()
        .filter(|(k, _)| observed(&baseline.entries[*k]))
        .map(|(_, n)| n)
        .sum();
    let baseline_unobserved = remaining
        .iter()
        .filter(|(k, _)| !observed(&baseline.entries[*k]))
        .map(|(_, n)| n)
        .sum();
    for (fp, entry) in &baseline.entries {
        if !observed(entry) {
            current
                .entry(fp.clone())
                .and_modify(|e| e.count = e.count.max(entry.count))
                .or_insert_with(|| entry.clone());
        }
    }
    let incomplete = steps
        .iter()
        .any(|s| matches!(s.status.as_str(), "failed" | "timeout" | "incomplete"));
    let weakening_tripped = diff_based.iter().any(|f| f.level >= p.policy.weakening);
    let total = all.len() + diff_based.len();
    let mut baseline_updated = false;
    if p.update_baseline && !incomplete && !weakening_tripped {
        let b = Baseline {
            version: 1,
            entries: current,
        };
        atomic_baseline(&bpath, &b)?;
        baseline_updated = true;
    }
    new.extend(diff_based);
    new.sort_by(|a, b| {
        b.level
            .cmp(&a.level)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    let tripped = new.iter().any(|f| enforced(f, p));
    if baseline_unobserved > 0 {
        limitations.push(format!("{baseline_unobserved} unobserved baseline entries are retained conservatively; they are not certified fixed by this run"));
    }
    if !p.policy.enforce_dead_code {
        limitations.push("dead-code findings are advisory by default, including SCIP findings: coverage does not prove safe deletion; opt in with --enforce-dead-code after review".into());
    }
    if !have_baseline {
        limitations.push(format!(
            "no baseline found at {}: every finding counts as new (create one with --update-baseline)",
            bpath.display()
        ));
    }
    Ok(VerifyReport {
        schema_version: 2,
        verdict: if incomplete {
            "incomplete"
        } else if weakening_tripped {
            "fail"
        } else if baseline_updated {
            "baseline-updated"
        } else if tripped {
            "fail"
        } else {
            "pass"
        }
        .into(),
        policy: p.policy.clone(),
        trusted_baseline: p.trusted_baseline.clone(),
        index,
        scip,
        baseline_unobserved,
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

fn enforced(f: &Finding, p: &VerifyParams) -> bool {
    let threshold = match f.category.as_str() {
        "dead" if !p.policy.enforce_dead_code => return false,
        "dead" => p.fail_on,
        "deps" => p.policy.dependencies,
        "weakening" => p.policy.weakening,
        "diagnostic" => p.policy.diagnostics,
        _ => p.policy.rules,
    };
    f.level >= threshold
}

fn parse_baseline(text: &str) -> Result<Baseline> {
    let b: Baseline = serde_json::from_str(text).context("invalid baseline JSON")?;
    ensure!(b.version == 1, "unsupported baseline version {}", b.version);
    ensure!(
        b.entries.values().all(|e| e.count > 0),
        "baseline counts must be positive"
    );
    Ok(b)
}

fn read_baseline(root: &Path, p: &VerifyParams) -> Result<(PathBuf, Baseline, bool)> {
    let bpath = match &p.baseline {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => root.join(path),
        None => root.join(BASELINE_FILE),
    };
    // Baseline writes must not escape the root through a path or symlink, including
    // a dangling symlink. MCP uses the same core check as the CLI.
    let parent = bpath
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.canonicalize()?.starts_with(root),
        "baseline must stay inside project root"
    );
    if let Ok(md) = std::fs::symlink_metadata(&bpath) {
        ensure!(
            !md.file_type().is_symlink(),
            "baseline symlinks are not supported"
        );
    }
    if let Some(rev) = &p.trusted_baseline {
        ensure!(
            !p.update_baseline,
            "trusted baseline mode forbids baseline maintenance"
        );
        ensure!(
            p.baseline.is_none(),
            "trusted baseline mode uses the committed default baseline only"
        );
        ensure!(
            matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit()),
            "trusted baseline must be an immutable full Git commit ID"
        );
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.pager=cat",
                "show",
                &format!("{rev}:{BASELINE_FILE}"),
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .context("reading trusted Git baseline")?;
        ensure!(
            out.status.success(),
            "cannot read baseline from trusted commit {}",
            rev
        );
        verify_trusted_allowlist(root, rev)?;
        let b = parse_baseline(std::str::from_utf8(&out.stdout)?)?;
        return Ok((bpath, b, true));
    }
    match std::fs::read_to_string(&bpath) {
        Ok(text) => Ok((bpath, parse_baseline(&text)?, true)),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && (p.baseline.is_none() || p.update_baseline) =>
        {
            Ok((bpath, Baseline::default(), false))
        }
        Err(e) => bail!("reading baseline {}: {}", bpath.display(), e),
    }
}

fn atomic_baseline(path: &Path, b: &Baseline) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp =
        tempfile::NamedTempFile::new_in(parent).context("creating baseline temporary file")?;
    writeln!(tmp, "{}", serde_json::to_string_pretty(b)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("replacing baseline {}", path.display()))?;
    Ok(())
}

/// A candidate allow-list is a policy input, not candidate evidence. Pinned mode
/// refuses any difference, including an ignored/untracked addition or symlink.
fn verify_trusted_allowlist(root: &Path, rev: &str) -> Result<()> {
    const ALLOW: &str = ".plumbgraph/allow.toml";
    let git = |args: &[&str]| -> Result<std::process::Output> {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["-c", "core.fsmonitor=false", "-c", "core.pager=cat"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()?;
        ensure!(out.status.success(), "cannot inspect trusted allow-list");
        Ok(out)
    };
    let tree = git(&["ls-tree", rev, "--", ALLOW])?;
    let expected = if tree.stdout.is_empty() {
        None
    } else {
        ensure!(
            tree.stdout.starts_with(b"100644 blob ") || tree.stdout.starts_with(b"100755 blob "),
            "trusted allow-list must be a regular file"
        );
        Some(git(&["show", &format!("{rev}:{ALLOW}")])?.stdout)
    };
    let path = root.join(ALLOW);
    if root.join(".plumbgraph").exists() {
        ensure!(
            !std::fs::symlink_metadata(root.join(".plumbgraph"))?
                .file_type()
                .is_symlink(),
            "trusted mode forbids a symlinked .plumbgraph directory"
        );
    }
    let actual = match std::fs::symlink_metadata(&path) {
        Ok(md) => {
            ensure!(
                md.is_file() && !md.file_type().is_symlink(),
                "trusted allow-list must be a regular file"
            );
            Some(std::fs::read(&path)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    ensure!(
        actual == expected,
        "allow-list differs from trusted baseline commit; review policy changes separately"
    );
    Ok(())
}
