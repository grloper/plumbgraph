//! High-level operations shared by the CLI and the MCP server.

use crate::deadcode::{dead_code, AllowList, DeadCodeOptions, Mode};
use crate::deps::{check_deps, DepsOptions, DepsReport};
use crate::index::{db_path_for, index_project, scan_manifests, IndexOptions, IndexStats};
use crate::registry::{HttpRegistry, Offline, Registry};
use crate::sanitize;
use crate::store::{Graph, Store};
use crate::weakening::{detect, WeakeningReport};
use crate::{Finding, Level};
use anyhow::{bail, Result};
use plumbgraph_langs::PackSet;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Built-in packs plus any packs in `$PLUMB_PACKS_DIR` or `~/.config/plumbgraph/packs`.
pub fn load_packs() -> Result<PackSet> {
    let mut set = PackSet::builtin()?;
    let dir = std::env::var_os("PLUMB_PACKS_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/plumbgraph/packs"))
        });
    if let Some(d) = dir {
        set = set.with_dir(&d)?;
    }
    Ok(set)
}

#[derive(Debug, Clone, Default)]
pub struct Target {
    /// Project root (directory to index).
    pub root: PathBuf,
    /// Override the database path (default `<root>/.plumbgraph/index.db`).
    pub db: Option<PathBuf>,
}

fn opts(t: &Target) -> Result<IndexOptions> {
    let mut o = IndexOptions::new(load_packs()?);
    o.db_path = t.db.clone();
    Ok(o)
}

pub fn index(t: &Target, force: bool) -> Result<IndexStats> {
    let mut o = opts(t)?;
    o.force = force;
    index_project(&t.root, &o)
}

fn open_graph(t: &Target) -> Result<(Graph, PathBuf, IndexStats)> {
    let stats = index(t, false)?;
    let root = t.root.canonicalize()?;
    let db = db_path_for(&root, &opts(t)?);
    let g = Graph::load(&Store::open(&db)?)?;
    Ok((g, root, stats))
}

#[derive(Debug, Clone)]
pub struct DeadCodeParams {
    pub library_mode: bool,
    pub min_confidence: f64,
    pub kinds: Option<Vec<String>>,
    pub allow_file: Option<PathBuf>,
    pub path_prefix: Option<String>,
    pub include_test_only: bool,
}

impl Default for DeadCodeParams {
    fn default() -> Self {
        DeadCodeParams {
            library_mode: false,
            min_confidence: 0.70,
            kinds: None,
            allow_file: None,
            path_prefix: None,
            include_test_only: true,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DeadCodeResult {
    pub findings: Vec<Finding>,
    pub summary: Summary,
    pub index: IndexStats,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct Summary {
    pub total: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

pub fn summarize(f: &[Finding]) -> Summary {
    let mut s = Summary {
        total: f.len(),
        ..Default::default()
    };
    for x in f {
        match x.level {
            Level::High => s.high += 1,
            Level::Medium => s.medium += 1,
            Level::Low => s.low += 1,
        }
    }
    s
}

pub fn run_dead_code(t: &Target, p: &DeadCodeParams) -> Result<DeadCodeResult> {
    let (g, root, stats) = open_graph(t)?;
    let mut o = DeadCodeOptions {
        mode: if p.library_mode { Mode::Lib } else { Mode::App },
        min_confidence: p.min_confidence,
        allow: AllowList::load(&root, p.allow_file.as_deref())?,
        path_prefix: p.path_prefix.clone(),
        report_test_only: p.include_test_only,
        ..Default::default()
    };
    if let Some(k) = &p.kinds {
        o.kinds = k.clone();
    }
    let findings = dead_code(&g, &o);
    Ok(DeadCodeResult {
        summary: summarize(&findings),
        findings,
        index: stats,
        limitations: vec![
            "tier-0 (tree-sitter, name-based) analysis: no type information, reflection or macro expansion".into(),
            "never delete on `medium` or `low` findings without checking usages yourself; `high` means the name occurs nowhere else in indexed code, not that deletion is proven safe".into(),
            "files excluded by .gitignore, vendor/build directories and files over 2 MiB are not indexed".into(),
        ],
    })
}

#[derive(Debug, Clone)]
pub struct DepsParams {
    pub offline: bool,
    pub path_prefix: Option<String>,
    pub min_confidence: f64,
    pub check_declared: bool,
}

impl Default for DepsParams {
    fn default() -> Self {
        DepsParams {
            offline: false,
            path_prefix: None,
            min_confidence: 0.0,
            check_declared: true,
        }
    }
}

pub fn run_check_deps(
    t: &Target,
    p: &DepsParams,
    registry_override: Option<&dyn Registry>,
) -> Result<DepsReport> {
    let (g, root, _) = open_graph(t)?;
    let manifests = scan_manifests(&root);
    let db = db_path_for(&root, &opts(t)?);
    let cache = db.parent().map(|d| d.join("registry-cache.json"));
    let o = DepsOptions {
        path_prefix: p.path_prefix.clone(),
        check_declared: p.check_declared,
        ..Default::default()
    };
    let mut rep = if let Some(r) = registry_override {
        check_deps(&g, &manifests, r, &o)
    } else if p.offline {
        check_deps(&g, &manifests, &Offline, &o)
    } else {
        let r = HttpRegistry::new(cache);
        let rep = check_deps(&g, &manifests, &r, &o);
        r.save_cache();
        rep
    };
    rep.findings.retain(|f| f.confidence >= p.min_confidence);
    Ok(rep)
}

pub fn run_weakening(repo: &Path, base: &str, head: Option<&str>) -> Result<WeakeningReport> {
    detect(repo, base, head)
}

#[derive(Debug, Serialize)]
pub struct SymbolHit {
    pub name: String,
    pub qname: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub exported: bool,
    pub source: &'static str,
    pub confidence: f64,
}

pub fn find_symbol(
    t: &Target,
    query: &str,
    kinds: &[String],
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    if query.is_empty() {
        bail!("empty query");
    }
    let (g, _, _) = open_graph(t)?;
    let q = query.to_lowercase();
    let mut hits: Vec<(u8, SymbolHit)> = vec![];
    for s in &g.symbols {
        if s.kind == "impl" || (!kinds.is_empty() && !kinds.contains(&s.kind)) {
            continue;
        }
        let (n, qn) = (s.name.to_lowercase(), s.qname.to_lowercase());
        let rank = if n == q || qn == q {
            0
        } else if n.starts_with(&q) {
            1
        } else if n.contains(&q) || qn.contains(&q) {
            2
        } else {
            continue;
        };
        let Some(f) = g.file(s.file_id) else { continue };
        hits.push((
            rank,
            SymbolHit {
                name: sanitize(&s.name),
                qname: sanitize(&s.qname),
                kind: s.kind.clone(),
                file: f.path.clone(),
                line: s.start_line,
                exported: s.exported,
                source: "t0-treesitter",
                confidence: 0.9,
            },
        ));
    }
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.file.cmp(&b.1.file))
            .then(a.1.line.cmp(&b.1.line))
    });
    Ok(hits
        .into_iter()
        .take(limit.clamp(1, 200))
        .map(|(_, h)| h)
        .collect())
}

#[derive(Debug, Serialize)]
pub struct RefHit {
    pub target: String,
    pub target_file: String,
    pub target_line: u32,
    pub from: String,
    pub file: String,
    pub line: u32,
    pub kind: String,
    pub confidence: f64,
    pub source: String,
}

pub fn references(
    t: &Target,
    symbol: &str,
    min_confidence: f64,
    limit: usize,
) -> Result<Vec<RefHit>> {
    let (g, _, _) = open_graph(t)?;
    let want = symbol.to_lowercase();
    let targets: Vec<&crate::store::SymRow> = g
        .symbols
        .iter()
        .filter(|s| {
            s.kind != "impl" && (s.qname.to_lowercase() == want || s.name.to_lowercase() == want)
        })
        .collect();
    if targets.is_empty() {
        bail!("no symbol named `{}`", sanitize(symbol));
    }
    let ids: std::collections::HashMap<i64, &crate::store::SymRow> =
        targets.iter().map(|s| (s.id, *s)).collect();
    let sym_by_id: std::collections::HashMap<i64, &crate::store::SymRow> =
        g.symbols.iter().map(|s| (s.id, s)).collect();
    let mut out = vec![];
    for e in &g.edges {
        let Some(tgt) = ids.get(&e.dst_symbol) else {
            continue;
        };
        if e.confidence < min_confidence {
            continue;
        }
        let file = g
            .file(e.src_file)
            .map(|f| f.path.clone())
            .unwrap_or_default();
        let tf = g
            .file(tgt.file_id)
            .map(|f| f.path.clone())
            .unwrap_or_default();
        let from = e
            .src_symbol
            .and_then(|s| sym_by_id.get(&s))
            .map(|s| sanitize(&s.qname))
            .unwrap_or_else(|| "<module level>".into());
        out.push(RefHit {
            target: sanitize(&tgt.qname),
            target_file: tf,
            target_line: tgt.start_line,
            from,
            file,
            line: e.line,
            kind: e.kind.clone(),
            confidence: (e.confidence * 100.0).round() / 100.0,
            source: e.source.clone(),
        });
    }
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out.truncate(limit.clamp(1, 500));
    Ok(out)
}
