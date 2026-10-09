//! Token-budgeted repo map (personalised PageRank over resolved edges) and impact analysis.
//!
//! Token counts are an *estimate* (`ceil(chars / 4)`); no tokenizer is bundled. The budget is
//! enforced against that estimate, so real token counts differ by model.

use crate::ops::{open_graph_scip, ScipOpts, Target};
use crate::sanitize;
use crate::store::Graph;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;

pub fn est_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

// ------------------------------------------------------------------ git helpers

fn git_out(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A revision must not look like a git option (argument injection from MCP clients).
fn check_rev(rev: &str) -> Result<()> {
    if rev.is_empty() || rev.starts_with('-') || rev.contains('\0') {
        bail!("invalid revision `{}`", sanitize(rev));
    }
    Ok(())
}

/// Files changed in the working tree vs `base` (plus untracked files), repo-relative to `root`.
pub fn changed_files(root: &Path, base: &str) -> Result<Vec<String>> {
    check_rev(base)?;
    let mut set: Vec<String> = vec![];
    for args in [
        vec!["diff", "--name-only", "--relative", base, "--"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        for l in git_out(root, &args)?.lines() {
            let l = l.trim();
            if !l.is_empty() && !l.starts_with(".plumbgraph/") && !set.iter().any(|x| x == l) {
                set.push(l.to_string());
            }
        }
    }
    set.sort();
    Ok(set)
}

/// New-file line ranges touched by `git diff -U0 base`, per file.
pub fn changed_ranges(root: &Path, base: &str) -> Result<BTreeMap<String, Vec<(u32, u32)>>> {
    check_rev(base)?;
    let diff = git_out(
        root,
        &["diff", "-U0", "--no-color", "--relative", base, "--"],
    )?;
    let mut map: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut cur: Option<String> = None;
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("+++ ") {
            cur = p.strip_prefix("b/").map(|s| s.to_string());
        } else if l.starts_with("@@") {
            // @@ -a,b +c,d @@
            let Some(plus) = l.split_whitespace().find(|t| t.starts_with('+')) else {
                continue;
            };
            let spec = &plus[1..];
            let (start, len) = match spec.split_once(',') {
                Some((a, b)) => (a.parse::<u32>().unwrap_or(0), b.parse::<u32>().unwrap_or(1)),
                None => (spec.parse::<u32>().unwrap_or(0), 1),
            };
            if let Some(f) = &cur {
                let end = if len == 0 { start } else { start + len - 1 };
                map.entry(f.clone())
                    .or_default()
                    .push((start.max(1), end.max(1)));
            }
        }
    }
    // untracked files: whole file
    for f in git_out(root, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        if f.starts_with(".plumbgraph/") {
            continue;
        }
        map.entry(f.to_string()).or_default().push((1, u32::MAX));
    }
    Ok(map)
}

// ------------------------------------------------------------------ ranking

struct Ranked {
    /// rank per symbol index in `g.symbols` (sums to ~1 over symbols+file nodes)
    sym_rank: Vec<f64>,
    in_degree: Vec<u32>,
}

fn pagerank(g: &Graph, personal_files: &HashSet<i64>) -> Ranked {
    let ns = g.symbols.len();
    let file_index: HashMap<i64, usize> = g
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.id, ns + i))
        .collect();
    let sym_index: HashMap<i64, usize> = g
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, i))
        .collect();
    let test_file: HashSet<i64> = g.files.iter().filter(|f| f.is_test).map(|f| f.id).collect();
    let n = ns + g.files.len();
    let mut out: Vec<Vec<(usize, f64)>> = vec![vec![]; n];
    let mut in_degree = vec![0u32; ns];
    for e in &g.edges {
        let Some(&dst) = sym_index.get(&e.dst_symbol) else {
            continue;
        };
        let src = match e.src_symbol.and_then(|s| sym_index.get(&s)) {
            Some(&s) => s,
            None => match file_index.get(&e.src_file) {
                Some(&f) => f,
                None => continue,
            },
        };
        if src == dst {
            continue;
        }
        // callers in test files count for less: tests should not decide what is "important"
        let w = e.confidence.max(0.05)
            * if test_file.contains(&e.src_file) {
                0.3
            } else {
                1.0
            };
        out[src].push((dst, w));
        in_degree[dst] += 1;
    }
    // teleport vector
    let mut tele = vec![1.0 / n.max(1) as f64; n];
    if !personal_files.is_empty() {
        let mut boosted = vec![0.0; n];
        let mut cnt = 0.0;
        for (i, s) in g.symbols.iter().enumerate() {
            if personal_files.contains(&s.file_id) {
                boosted[i] = 1.0;
                cnt += 1.0;
            }
        }
        if cnt > 0.0 {
            for i in 0..n {
                tele[i] = 0.5 * tele[i] + 0.5 * boosted[i] / cnt;
            }
        }
    }
    let d = 0.85;
    let mut rank = tele.clone();
    for _ in 0..50 {
        let mut next = vec![0.0; n];
        let mut dangling = 0.0;
        for i in 0..n {
            let tot: f64 = out[i].iter().map(|x| x.1).sum();
            if tot <= 0.0 {
                dangling += rank[i];
            } else {
                for &(j, w) in &out[i] {
                    next[j] += d * rank[i] * w / tot;
                }
            }
        }
        let mut delta = 0.0;
        for i in 0..n {
            next[i] += (1.0 - d) * tele[i] + d * dangling * tele[i];
            delta += (next[i] - rank[i]).abs();
        }
        rank = next;
        if delta < 1e-9 {
            break;
        }
    }
    Ranked {
        sym_rank: rank[..ns].to_vec(),
        in_degree,
    }
}

// ------------------------------------------------------------------ map

#[derive(Debug, Clone)]
pub struct MapParams {
    /// Hard cap on the estimated token size of `text`.
    pub tokens: usize,
    /// Boost symbols of files changed vs this revision (e.g. `HEAD`).
    pub changed: Option<String>,
    /// Extra files (project-relative) to focus on.
    pub focus: Vec<String>,
    pub include_tests: bool,
    pub scip: ScipOpts,
}

impl Default for MapParams {
    fn default() -> Self {
        MapParams {
            tokens: 1500,
            changed: None,
            focus: vec![],
            include_tests: false,
            scip: ScipOpts::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MapSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub rank: f64,
    pub references: u32,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapFile {
    pub path: String,
    pub rank: f64,
    pub changed: bool,
    pub symbols: Vec<MapSymbol>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapResult {
    pub text: String,
    pub tokens_estimated: usize,
    pub budget: usize,
    pub token_estimate_method: &'static str,
    pub total_symbols: usize,
    pub shown_symbols: usize,
    pub changed_files: Vec<String>,
    pub files: Vec<MapFile>,
    /// `scip` when a SCIP index refined the edges, else `t0-treesitter`
    pub edge_source: String,
}

impl MapResult {
    pub fn symbol_rank(&self, name: &str) -> Option<f64> {
        self.files
            .iter()
            .flat_map(|f| f.symbols.iter())
            .find(|s| s.name == name)
            .map(|s| s.rank)
    }
}

const MAP_KINDS: &[&str] = &[
    "function",
    "method",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "type",
    "const",
];

fn signature(
    root: &Path,
    file: &str,
    line: u32,
    cache: &mut HashMap<String, Vec<String>>,
) -> String {
    let lines = cache.entry(file.to_string()).or_insert_with(|| {
        std::fs::read_to_string(root.join(file))
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default()
    });
    // join up to 4 lines until the declaration header ends (`{`, `;` or a trailing `:`)
    let mut joined = String::new();
    for l in lines.iter().skip(line.saturating_sub(1) as usize).take(4) {
        let t = l.trim();
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(t);
        if t.ends_with('{') || t.ends_with(';') || t.ends_with(':') || t.contains(" {") {
            break;
        }
    }
    let cut = joined.split(" {").next().unwrap_or(&joined);
    let l = cut.trim_end_matches(['{', ':', ' ']);
    let mut sig = sanitize(l);
    if sig.chars().count() > 120 {
        sig = sig.chars().take(120).collect::<String>() + "…";
    }
    sig
}

pub fn run_map(t: &Target, p: &MapParams) -> Result<MapResult> {
    let (g, root, _, scip) = open_graph_scip(t, &p.scip)?;
    let changed_files = match &p.changed {
        Some(base) => changed_files(&root, base)?,
        None => vec![],
    };
    let mut focus: HashSet<String> = changed_files.iter().cloned().collect();
    focus.extend(p.focus.iter().cloned());
    let personal: HashSet<i64> = g
        .files
        .iter()
        .filter(|f| focus.contains(&f.path))
        .map(|f| f.id)
        .collect();
    let ranked = pagerank(&g, &personal);
    let file_by_id: HashMap<i64, &crate::store::FileRow> =
        g.files.iter().map(|f| (f.id, f)).collect();

    let mut cands: Vec<usize> = (0..g.symbols.len())
        .filter(|&i| {
            let s = &g.symbols[i];
            MAP_KINDS.contains(&s.kind.as_str())
                && !s.name.is_empty()
                && file_by_id
                    .get(&s.file_id)
                    .map(|f| (p.include_tests || !(f.is_test || s.is_test)) && !f.is_generated)
                    .unwrap_or(false)
        })
        .collect();
    cands.sort_by(|&a, &b| {
        ranked.sym_rank[b]
            .partial_cmp(&ranked.sym_rank[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(g.symbols[a].qname.cmp(&g.symbols[b].qname))
    });
    let total_symbols = cands.len();

    let header_reserve = est_tokens(
        "# plumb map: 999 files, 99999 of 99999 symbols (~99999 tokens, budget 99999)\n",
    );
    let mut used = header_reserve;
    let mut shown_files: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut sig_cache = HashMap::new();
    for &i in &cands {
        let s = &g.symbols[i];
        let f = file_by_id[&s.file_id];
        let sig = signature(&root, &f.path, s.start_line, &mut sig_cache);
        let line = format!("  {}: {}\n", s.start_line, sig);
        let mut cost = est_tokens(&line);
        if !shown_files.contains_key(&f.path) {
            cost += est_tokens(&format!("{}\n", f.path));
        }
        if used + cost > p.tokens {
            continue;
        }
        used += cost;
        shown_files
            .entry(f.path.clone())
            .or_default()
            .push((i, sig));
    }
    let mut files: Vec<MapFile> = vec![];
    for (path, mut syms) in shown_files {
        syms.sort_by_key(|(i, _)| g.symbols[*i].start_line);
        let rank = syms.iter().map(|(i, _)| ranked.sym_rank[*i]).sum();
        files.push(MapFile {
            changed: changed_files.contains(&path),
            path,
            rank,
            symbols: syms
                .into_iter()
                .map(|(i, sig)| {
                    let s = &g.symbols[i];
                    MapSymbol {
                        name: sanitize(&s.name),
                        kind: s.kind.clone(),
                        line: s.start_line,
                        rank: ranked.sym_rank[i],
                        references: ranked.in_degree[i],
                        signature: sig,
                    }
                })
                .collect(),
        });
    }
    files.sort_by(|a, b| {
        b.rank
            .partial_cmp(&a.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.path.cmp(&b.path))
    });
    let shown_symbols: usize = files.iter().map(|f| f.symbols.len()).sum();
    let mut body = String::new();
    for f in &files {
        body.push_str(&format!(
            "{}{}\n",
            f.path,
            if f.changed { " (changed)" } else { "" }
        ));
        for s in &f.symbols {
            body.push_str(&format!("  {}: {}\n", s.line, s.signature));
        }
    }
    let mut text = format!(
        "# plumb map: {} files, {} of {} symbols (~{} tokens, budget {})\n",
        files.len(),
        shown_symbols,
        total_symbols,
        0,
        p.tokens
    );
    text.push_str(&body);
    let tokens_estimated = est_tokens(&text);
    text = text.replacen("(~0 tokens", &format!("(~{tokens_estimated} tokens"), 1);
    let tokens_estimated = est_tokens(&text);
    Ok(MapResult {
        text,
        tokens_estimated,
        budget: p.tokens,
        token_estimate_method: "ceil(chars/4); not a real tokenizer",
        total_symbols,
        shown_symbols,
        changed_files,
        files,
        edge_source: if scip.is_some() {
            "scip+t0-treesitter"
        } else {
            "t0-treesitter"
        }
        .into(),
    })
}

// ------------------------------------------------------------------ impact

#[derive(Debug, Clone)]
pub enum ImpactTarget {
    Symbol(String),
    /// Working tree vs this revision.
    Diff(String),
}

#[derive(Debug, Clone)]
pub struct ImpactParams {
    pub target: ImpactTarget,
    pub depth: u32,
    pub min_confidence: f64,
    pub scip: ScipOpts,
}

impl Default for ImpactParams {
    fn default() -> Self {
        ImpactParams {
            target: ImpactTarget::Diff("HEAD".into()),
            depth: 4,
            min_confidence: 0.3,
            scip: ScipOpts::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Affected {
    pub name: String,
    pub qname: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub distance: u32,
    /// product of edge confidences along the best path found
    pub confidence: f64,
    pub source: String,
    pub via: String,
    pub is_test: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactResult {
    pub seeds: Vec<String>,
    pub affected: Vec<Affected>,
    pub affected_files: Vec<String>,
    pub tests_to_run: Vec<String>,
    /// Files that reference a seed from module level (no enclosing symbol)
    pub module_level_dependents: Vec<String>,
    pub limitations: Vec<String>,
}

/// (enclosing symbol, file id, edge confidence, edge source)
type Caller<'a> = (Option<usize>, i64, f64, &'a str);

pub fn run_impact(t: &Target, p: &ImpactParams) -> Result<ImpactResult> {
    let (g, root, _, _) = open_graph_scip(t, &p.scip)?;
    let file_path: HashMap<i64, &str> = g.files.iter().map(|f| (f.id, f.path.as_str())).collect();
    let file_is_test: HashMap<i64, bool> = g.files.iter().map(|f| (f.id, f.is_test)).collect();
    let mut seeds: Vec<usize> = vec![];
    match &p.target {
        ImpactTarget::Symbol(q) => {
            let ql = q.to_lowercase();
            for (i, s) in g.symbols.iter().enumerate() {
                if s.kind != "impl" && (s.name.to_lowercase() == ql || s.qname.to_lowercase() == ql)
                {
                    seeds.push(i);
                }
            }
            if seeds.is_empty() {
                bail!("no symbol named `{q}` in the index");
            }
        }
        ImpactTarget::Diff(base) => {
            let ranges = changed_ranges(&root, base)?;
            for (i, s) in g.symbols.iter().enumerate() {
                if s.kind == "impl" {
                    continue;
                }
                let Some(path) = file_path.get(&s.file_id) else {
                    continue;
                };
                if let Some(rs) = ranges.get(*path) {
                    if rs
                        .iter()
                        .any(|&(a, b)| a <= s.end_line && b >= s.start_line)
                    {
                        seeds.push(i);
                    }
                }
            }
            // keep only the innermost symbol when a container and its member both overlap
            let keep: Vec<usize> = seeds
                .iter()
                .copied()
                .filter(|&i| {
                    !seeds.iter().any(|&j| {
                        j != i
                            && g.symbols[j].file_id == g.symbols[i].file_id
                            && g.symbols[j].parent_idx == Some(g.symbols[i].idx)
                    })
                })
                .collect();
            seeds = keep;
        }
    }
    let sym_index: HashMap<i64, usize> = g
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, i))
        .collect();
    let mut rev: HashMap<usize, Vec<Caller>> = HashMap::new();
    for e in &g.edges {
        if e.confidence < p.min_confidence {
            continue;
        }
        if let Some(&d) = sym_index.get(&e.dst_symbol) {
            let s = e.src_symbol.and_then(|s| sym_index.get(&s).copied());
            rev.entry(d)
                .or_default()
                .push((s, e.src_file, e.confidence, e.source.as_str()));
        }
    }
    let seed_set: HashSet<usize> = seeds.iter().copied().collect();
    let mut seen: HashMap<usize, (u32, f64, String, String)> = HashMap::new();
    let mut module_level: HashSet<String> = HashSet::new();
    let mut q: VecDeque<(usize, u32, f64)> = seeds.iter().map(|&s| (s, 0, 1.0)).collect();
    while let Some((cur, dist, conf)) = q.pop_front() {
        if dist >= p.depth {
            continue;
        }
        let Some(callers) = rev.get(&cur) else {
            continue;
        };
        for &(src, src_file, c, source) in callers {
            match src {
                Some(si) => {
                    if seed_set.contains(&si) {
                        continue;
                    }
                    let nc = conf * c;
                    if nc < p.min_confidence {
                        continue;
                    }
                    match seen.get(&si) {
                        Some((d, oc, _, _)) if *d < dist + 1 || (*d == dist + 1 && *oc >= nc) => {}
                        _ => {
                            seen.insert(
                                si,
                                (
                                    dist + 1,
                                    nc,
                                    source.to_string(),
                                    g.symbols[cur].qname.clone(),
                                ),
                            );
                            q.push_back((si, dist + 1, nc));
                        }
                    }
                }
                None => {
                    if let Some(p) = file_path.get(&src_file) {
                        module_level.insert(p.to_string());
                    }
                }
            }
        }
    }
    let mut affected: Vec<Affected> = seen
        .into_iter()
        .map(|(i, (dist, conf, source, via))| {
            let s = &g.symbols[i];
            Affected {
                name: sanitize(&s.name),
                qname: sanitize(&s.qname),
                kind: s.kind.clone(),
                file: file_path.get(&s.file_id).unwrap_or(&"").to_string(),
                line: s.start_line,
                distance: dist,
                confidence: (conf * 100.0).round() / 100.0,
                source,
                via: sanitize(&via),
                is_test: s.is_test || *file_is_test.get(&s.file_id).unwrap_or(&false),
            }
        })
        .collect();
    affected.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    let mut files: Vec<String> = affected.iter().map(|a| a.file.clone()).collect();
    files.extend(module_level.iter().cloned());
    files.sort();
    files.dedup();
    let mut tests: Vec<String> = affected
        .iter()
        .filter(|a| a.is_test)
        .map(|a| a.file.clone())
        .collect();
    for f in &module_level {
        if g.files.iter().any(|x| &x.path == f && x.is_test) {
            tests.push(f.clone());
        }
    }
    tests.sort();
    tests.dedup();
    let seed_names: Vec<String> = seeds
        .iter()
        .map(|&i| sanitize(&g.symbols[i].name))
        .collect();
    Ok(ImpactResult {
        seeds: seed_names,
        affected,
        affected_files: files,
        tests_to_run: tests,
        module_level_dependents: module_level.into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect(),
        limitations: vec![
            "reverse reachability over resolved call/reference edges: dynamic dispatch, reflection and code outside the index are invisible".into(),
            "confidence is the product of edge confidences along the best path found; it is a heuristic, not a probability".into(),
            "tests_to_run lists test files that reach a changed symbol through the graph; absence is not proof a test is unaffected".into(),
        ],
    })
}
