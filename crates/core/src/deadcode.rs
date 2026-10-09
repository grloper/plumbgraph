//! Dead-code analysis: reachability from entry points over the tier-0 reference graph,
//! then a confidence score per unreachable symbol.
//!
//! Confidence model (documented in docs/DEAD_CODE.md):
//! `score = clamp((0.70 + bonuses) * (1 - sum(penalties)), 0, 0.99)`
//! bonuses: private symbol +0.10, name occurs nowhere else in the indexed code +0.15.
//! penalties: dynamic-access markers in file 0.20, name appears in a string literal 0.15,
//! decorated/attributed 0.30, exported (app mode) 0.10, imported-by-name elsewhere 0.20,
//! file has syntax errors 0.10.
//! Levels: high >= 0.90, medium >= 0.70, low otherwise (hidden by default).

use crate::store::{Graph, SymRow};
use crate::{sanitize, Finding, Level, Severity};
use anyhow::{Context, Result};
use globset::Glob;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Exported symbols are *not* assumed to be used externally (lower confidence only).
    App,
    /// Exported symbols are public API and are treated as entry points.
    Lib,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AllowEntry {
    pub symbol: Option<String>,
    pub path: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AllowList {
    #[serde(default)]
    pub allow: Vec<AllowEntry>,
}

impl AllowList {
    pub fn parse(text: &str) -> Result<Self> {
        let a: AllowList = toml::from_str(text).context("parsing allow-list TOML")?;
        for (i, e) in a.allow.iter().enumerate() {
            if e.symbol.is_none() && e.path.is_none() {
                anyhow::bail!("allow entry #{} needs `symbol` and/or `path`", i + 1);
            }
        }
        Ok(a)
    }

    /// Load `<root>/.plumbgraph/allow.toml` (if present) plus an optional extra file.
    pub fn load(root: &Path, extra: Option<&Path>) -> Result<Self> {
        let mut all = AllowList::default();
        let default = root.join(".plumbgraph").join("allow.toml");
        for p in [Some(default), extra.map(|p| p.to_path_buf())]
            .into_iter()
            .flatten()
        {
            if p.is_file() {
                let t = std::fs::read_to_string(&p)
                    .with_context(|| format!("reading {}", p.display()))?;
                all.allow.extend(
                    Self::parse(&t)
                        .with_context(|| p.display().to_string())?
                        .allow,
                );
            }
        }
        Ok(all)
    }

    fn matches(&self, name: &str, qname: &str, path: &str) -> Option<String> {
        for e in &self.allow {
            let sym_ok = e
                .symbol
                .as_ref()
                .map(|g| {
                    Glob::new(g)
                        .map(|g| {
                            let m = g.compile_matcher();
                            m.is_match(name) || m.is_match(qname)
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(true);
            let path_ok = e
                .path
                .as_ref()
                .map(|g| {
                    Glob::new(g)
                        .map(|g| g.compile_matcher().is_match(path))
                        .unwrap_or(false)
                })
                .unwrap_or(true);
            if sym_ok && path_ok {
                return Some(e.reason.clone().unwrap_or_else(|| "allow-listed".into()));
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
pub struct DeadCodeOptions {
    pub mode: Mode,
    pub min_confidence: f64,
    /// Symbol kinds to report. Default: function, method, class, struct, enum, trait, interface, type, const.
    pub kinds: Vec<String>,
    pub allow: AllowList,
    /// Only report symbols whose file path starts with this prefix.
    pub path_prefix: Option<String>,
    /// Also report symbols used only by tests (separate rule).
    pub report_test_only: bool,
}

impl Default for DeadCodeOptions {
    fn default() -> Self {
        DeadCodeOptions {
            mode: Mode::App,
            min_confidence: 0.70,
            kinds: [
                "function",
                "method",
                "class",
                "struct",
                "enum",
                "trait",
                "interface",
                "type",
                "const",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            allow: AllowList::default(),
            path_prefix: None,
            report_test_only: true,
        }
    }
}

struct Reach {
    reachable: Vec<bool>,
}

fn reach(
    g: &Graph,
    adj: &[Vec<usize>],
    parent_of: &[Option<usize>],
    roots: Vec<usize>,
    cond_entries: &[(usize, String)],
    by_name: &HashMap<&str, Vec<usize>>,
) -> Reach {
    let n = g.symbols.len();
    let mut seen = vec![false; n];
    let mut stack: Vec<usize> = vec![];
    let mark = |i: usize, seen: &mut Vec<bool>, stack: &mut Vec<usize>| {
        // marking a symbol also marks its enclosing symbols (a used member keeps its container used)
        let mut cur = Some(i);
        while let Some(c) = cur {
            if !seen[c] {
                seen[c] = true;
                stack.push(c);
            }
            cur = parent_of[c];
        }
    };
    for r in roots {
        mark(r, &mut seen, &mut stack);
    }
    loop {
        while let Some(s) = stack.pop() {
            for &d in &adj[s] {
                mark(d, &mut seen, &mut stack);
            }
        }
        let mut changed = false;
        for (i, cond) in cond_entries {
            if seen[*i] {
                continue;
            }
            let met = if cond == "parent" {
                parent_of[*i].map(|p| seen[p]).unwrap_or(true)
            } else if let Some(t) = cond.strip_prefix("type:") {
                match by_name.get(t) {
                    None => true, // foreign type: the trait impl is invoked from elsewhere
                    Some(v) => {
                        v.iter().any(|&j| {
                            seen[j]
                                && matches!(
                                    g.symbols[j].kind.as_str(),
                                    "struct" | "enum" | "class" | "trait" | "type" | "interface"
                                )
                        }) || !v.iter().any(|&j| {
                            matches!(
                                g.symbols[j].kind.as_str(),
                                "struct" | "enum" | "class" | "trait" | "type" | "interface"
                            )
                        })
                    }
                }
            } else {
                true
            };
            if met {
                mark(*i, &mut seen, &mut stack);
                changed = true;
            }
        }
        if !changed && stack.is_empty() {
            break;
        }
    }
    Reach { reachable: seen }
}

pub fn dead_code(g: &Graph, opts: &DeadCodeOptions) -> Vec<Finding> {
    let n = g.symbols.len();
    let file_by_id: HashMap<i64, &crate::store::FileRow> =
        g.files.iter().map(|f| (f.id, f)).collect();
    let idx_of_id: HashMap<i64, usize> = g
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, i))
        .collect();
    let mut by_file_idx: HashMap<(i64, i64), usize> = HashMap::new();
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in g.symbols.iter().enumerate() {
        by_file_idx.insert((s.file_id, s.idx), i);
        by_name.entry(s.name.as_str()).or_default().push(i);
    }
    let parent_of: Vec<Option<usize>> = g
        .symbols
        .iter()
        .map(|s| {
            s.parent_idx
                .and_then(|p| by_file_idx.get(&(s.file_id, p)).copied())
        })
        .collect();
    let sym_is_test = |i: usize| {
        g.symbols[i].is_test
            || file_by_id
                .get(&g.symbols[i].file_id)
                .map(|f| f.is_test)
                .unwrap_or(false)
    };

    // adjacency (edges whose source is a symbol) + module-level edges
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
    let mut prod_module_dst: Vec<usize> = vec![];
    let mut test_module_dst: Vec<usize> = vec![];
    let mut inbound: Vec<usize> = vec![0; n];
    for e in &g.edges {
        let Some(&d) = idx_of_id.get(&e.dst_symbol) else {
            continue;
        };
        match e.src_symbol.and_then(|s| idx_of_id.get(&s).copied()) {
            Some(s) => {
                adj[s].push(d);
                if s != d {
                    inbound[d] += 1;
                }
            }
            None => {
                inbound[d] += 1;
                if file_by_id
                    .get(&e.src_file)
                    .map(|f| f.is_test)
                    .unwrap_or(false)
                {
                    test_module_dst.push(d);
                } else {
                    prod_module_dst.push(d);
                }
            }
        }
    }

    let mut html_names: HashSet<&str> = HashSet::new();
    for f in g.files.iter().filter(|f| f.lang == "html") {
        if let Some(v) = g.strings.get(&f.id) {
            html_names.extend(v.iter().map(|s| s.as_str()));
        }
    }
    // roots
    let mut prod_roots: Vec<usize> = vec![];
    let mut test_roots: Vec<usize> = vec![];
    let mut cond_entries: Vec<(usize, String)> = vec![];
    let mut allow_reason: HashMap<usize, String> = HashMap::new();
    for (i, s) in g.symbols.iter().enumerate() {
        let f = file_by_id.get(&s.file_id);
        let path = f.map(|f| f.path.as_str()).unwrap_or("");
        if sym_is_test(i) {
            test_roots.push(i);
            continue;
        }
        if s.keep.is_some() {
            prod_roots.push(i);
            continue;
        }
        if let Some(r) = opts.allow.matches(&s.name, &s.qname, path) {
            allow_reason.insert(i, r);
            prod_roots.push(i);
            continue;
        }
        if s.kind == "variable" && s.parent_idx.is_none() {
            // module-level statements execute on import: their references are live
            prod_roots.push(i);
            continue;
        }
        if s.parent_idx.is_none() && html_names.contains(s.name.as_str()) {
            // referenced from an .html file (inline <script>, onclick handlers, ...)
            prod_roots.push(i);
            continue;
        }
        if s.entry.is_some() {
            match &s.entry_cond {
                None => prod_roots.push(i),
                Some(c) => cond_entries.push((i, c.clone())),
            }
            continue;
        }
        if opts.mode == Mode::Lib && s.exported {
            prod_roots.push(i);
        }
    }
    prod_roots.extend(prod_module_dst.iter().copied());
    // test-only reachability starts from prod roots too
    let prod = reach(
        g,
        &adj,
        &parent_of,
        prod_roots.clone(),
        &cond_entries,
        &by_name,
    );
    let mut all_roots = prod_roots;
    all_roots.extend(test_roots);
    all_roots.extend(test_module_dst);
    let all = reach(g, &adj, &parent_of, all_roots, &cond_entries, &by_name);

    // name census
    let mut refs_by_name: HashMap<&str, Vec<(i64, u32)>> = HashMap::new();
    for r in &g.refs {
        refs_by_name
            .entry(r.name.as_str())
            .or_default()
            .push((r.file_id, r.line));
    }
    let mut string_names: HashSet<&str> = HashSet::new();
    for v in g.strings.values() {
        for s in v {
            string_names.insert(s.as_str());
        }
    }
    let mut imported_names: HashSet<&str> = HashSet::new();
    for i in &g.imports {
        // imports inside test files do not make a name look "re-exported"
        if file_by_id
            .get(&i.file_id)
            .map(|f| f.is_test)
            .unwrap_or(false)
        {
            continue;
        }
        for nme in &i.names {
            imported_names.insert(nme.as_str());
        }
    }

    let mut out: Vec<Finding> = vec![];
    for (i, s) in g.symbols.iter().enumerate() {
        let Some(f) = file_by_id.get(&s.file_id) else {
            continue;
        };
        if sym_is_test(i) || f.is_generated || s.keep.is_some() || allow_reason.contains_key(&i) {
            continue;
        }
        if !opts.kinds.iter().any(|k| k == &s.kind) || s.entry.is_some() {
            continue;
        }
        if let Some(p) = &opts.path_prefix {
            if !f.path.starts_with(p.as_str()) {
                continue;
            }
        }
        let dead = !all.reachable[i];
        let test_only = all.reachable[i] && !prod.reachable[i];
        if !(dead || (test_only && opts.report_test_only)) {
            continue;
        }
        out.push(score(
            g,
            opts,
            s,
            f.path.as_str(),
            &f.dynamic,
            &f.parse_status,
            f.id,
            dead,
            inbound[i],
            &refs_by_name,
            &string_names,
            &imported_names,
        ));
    }
    out.retain(|f| f.confidence >= opts.min_confidence);
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn score(
    _g: &Graph,
    opts: &DeadCodeOptions,
    s: &SymRow,
    path: &str,
    dynamic: &[String],
    parse_status: &str,
    file_id: i64,
    dead: bool,
    inbound: usize,
    refs_by_name: &HashMap<&str, Vec<(i64, u32)>>,
    string_names: &HashSet<&str>,
    imported_names: &HashSet<&str>,
) -> Finding {
    let name = s.name.as_str();
    let other_refs = refs_by_name
        .get(name)
        .map(|v| {
            v.iter()
                .filter(|(f, l)| !(*f == file_id && *l >= s.start_line && *l <= s.end_line))
                .count()
        })
        .unwrap_or(0);
    let in_string = string_names.contains(name);
    let imported = imported_names.contains(name);
    let census_zero = other_refs == 0 && !in_string && !imported;

    let mut bonus: f64 = 0.0;
    let mut pen: f64 = 0.0;
    let mut evidence: Vec<String> = vec![];
    let mut fp: Vec<String> = vec![];
    if dead {
        if inbound == 0 {
            evidence.push("no reference edges from any code (0 incoming)".to_string());
        } else {
            evidence.push(format!(
                "{inbound} reference(s) found, but only from code that is itself unreachable"
            ));
        }
        evidence.push("not reachable from entry points (main, tests, module-level code, exported API in --mode lib, keep/allow-list)".to_string());
    } else {
        evidence.push("reachable only from tests (no production code reaches it)".to_string());
    }
    if !s.exported {
        bonus += 0.10;
        evidence.push("private to its module/crate (complete scope visibility)".into());
    }
    if census_zero {
        bonus += 0.15;
        evidence.push(format!("the name `{}` occurs nowhere else in the indexed code (identifiers, strings, import lists)", sanitize(name)));
    }
    if !dynamic.is_empty() {
        pen += 0.20;
        fp.push(format!(
            "dynamic-access markers in this file: {}",
            dynamic.join(", ")
        ));
    }
    if in_string {
        pen += 0.15;
        fp.push(
            "the name appears in a string literal or a config/html file (possible dynamic lookup)"
                .into(),
        );
    }
    if s.decorated {
        pen += 0.30;
        fp.push("decorated/attributed: a framework may invoke it".into());
    }
    if s.subclass {
        pen += 0.25;
        fp.push("method of a class that extends/implements another type: it may override or implement a base/framework method that is called from outside the indexed code".into());
    }
    if s.exported {
        pen += 0.10;
        fp.push("exported/public: may be used by consumers outside the indexed code".into());
    }
    if imported && !in_string {
        pen += 0.20;
        fp.push("the name appears in an import/export list elsewhere (possible re-export)".into());
    }
    if parse_status == "syntax-errors" {
        pen += 0.10;
        fp.push("the file has syntax errors; extraction may be incomplete".into());
    }
    fp.push("tier-0 analysis is name-based: it does not see reflection, macros, or code outside the indexed files".into());
    let mut conf = ((0.70 + bonus) * (1.0 - pen)).clamp(0.0, 0.99);
    if !dead {
        conf = conf.min(0.85);
    }
    let rule = if dead {
        "dead-code"
    } else {
        "used-only-by-tests"
    };
    let msg = if dead {
        format!("{} `{}` appears unused", s.kind, sanitize(&s.qname))
    } else {
        format!("{} `{}` is only used by tests", s.kind, sanitize(&s.qname))
    };
    let mut f = Finding::new("dead", rule, path, s.start_line, conf, "t0-treesitter", msg);
    f.symbol = Some(sanitize(&s.qname));
    f.kind = s.kind.clone();
    f.severity = if f.level == Level::Low {
        Severity::Info
    } else {
        Severity::Warning
    };
    f.evidence = evidence;
    f.fp_risks = fp;
    let _ = opts;
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_parse_and_match() {
        let a = AllowList::parse("[[allow]]\nsymbol=\"legacy_*\"\nreason=\"public\"\n[[allow]]\npath=\"src/compat/**\"\n").unwrap();
        assert_eq!(
            a.matches("legacy_a", "legacy_a", "x.py").as_deref(),
            Some("public")
        );
        assert!(a.matches("other", "other", "x.py").is_none());
        assert!(a.matches("other", "other", "src/compat/a.py").is_some());
        assert!(AllowList::parse("[[allow]]\nreason=\"x\"\n").is_err());
    }
}
