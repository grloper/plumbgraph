//! SCIP ingestion: precise (compiler-grade) definitions and references from index files
//! produced by rust-analyzer (`rust-analyzer scip`), scip-typescript, scip-python, ...
//!
//! The SCIP index is an *overlay* on the tier-0 graph, applied **per occurrence**: for a source
//! file the index covers (and that has not changed since the index was written), a name-based
//! edge `(file, line, name)` is replaced by the SCIP-resolved edge only when the index reports
//! an occurrence of that name on that line that it resolved (to a project symbol, or to a
//! symbol defined outside the project). A name-based edge for which the index has no resolved
//! occurrence is kept: indexers cannot resolve calls on untyped receivers (plain JS,
//! unannotated Python), and dropping those edges would report live code as dead. Files and
//! symbols the index does not cover keep all name-based edges. Nothing here executes project
//! code or indexers; it only reads `.scip` files.

use crate::store::{EdgeRow, Graph, SymRow};
use anyhow::{bail, Context, Result};
use protobuf::Message;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Largest index file we are willing to parse.
pub const MAX_INDEX_BYTES: u64 = 512 * 1024 * 1024;
/// Confidence of an edge resolved by a SCIP indexer.
pub const SCIP_EDGE_CONFIDENCE: f64 = 0.97;
const ROLE_DEFINITION: i32 = 0x1;
const ROLE_IMPORT: i32 = 0x2;

#[derive(Debug, Clone)]
pub struct Occ {
    pub symbol: String,
    /// 1-based
    pub line: u32,
    pub is_def: bool,
    pub is_import: bool,
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub path: String,
    pub occs: Vec<Occ>,
}

#[derive(Debug, Clone, Default)]
pub struct ScipIndex {
    pub path: PathBuf,
    pub tool: String,
    pub mtime: Option<SystemTime>,
    pub docs: Vec<Doc>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ScipStats {
    pub indexes: Vec<String>,
    pub tools: Vec<String>,
    pub documents: usize,
    pub documents_matched: usize,
    pub documents_unmatched: usize,
    /// documents whose source file is newer than the index (ignored, name-based fallback)
    pub stale_files: usize,
    pub symbols_mapped: usize,
    pub edges_added: usize,
    pub edges_replaced: usize,
}

/// Parse a `.scip` file.
pub fn load(path: &Path) -> Result<ScipIndex> {
    let md = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if md.len() > MAX_INDEX_BYTES {
        bail!(
            "{} is larger than {} MiB; refusing to load it",
            path.display(),
            MAX_INDEX_BYTES >> 20
        );
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let idx = scip::types::Index::parse_from_bytes(&bytes)
        .with_context(|| format!("{} is not a valid SCIP index", path.display()))?;
    let tool = idx
        .metadata
        .as_ref()
        .and_then(|m| m.tool_info.as_ref())
        .map(|t| format!("{} {}", t.name, t.version).trim().to_string())
        .unwrap_or_default();
    let docs = idx
        .documents
        .iter()
        .map(|d| Doc {
            path: d.relative_path.replace('\\', "/"),
            occs: d
                .occurrences
                .iter()
                .filter(|o| !o.range.is_empty() && !o.symbol.is_empty())
                .map(|o| Occ {
                    symbol: o.symbol.clone(),
                    line: o.range[0].max(0) as u32 + 1,
                    is_def: o.symbol_roles & ROLE_DEFINITION != 0,
                    is_import: o.symbol_roles & ROLE_IMPORT != 0,
                })
                .collect(),
        })
        .collect();
    Ok(ScipIndex {
        path: path.to_path_buf(),
        tool,
        mtime: md.modified().ok(),
        docs,
    })
}

/// Index files found without being told: `<root>/index.scip`, `<root>/.plumbgraph/index.scip`.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    [
        root.join("index.scip"),
        root.join(".plumbgraph").join("index.scip"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// Last descriptor name of a global SCIP symbol (`pkg/Class#method().` -> `method`).
pub fn symbol_name(sym: &str) -> Option<String> {
    if scip::symbol::is_local_symbol(sym) {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(sym).ok()?;
    let d = parsed.descriptors.last()?;
    if d.name.is_empty() {
        None
    } else {
        Some(d.name.clone())
    }
}

fn innermost<'a>(syms: &'a [&'a SymRow], line: u32, name: Option<&str>) -> Option<&'a SymRow> {
    syms.iter()
        .copied()
        .filter(|s| {
            s.kind != "impl"
                && s.start_line <= line
                && line <= s.end_line
                && name.map(|n| s.name == n).unwrap_or(true)
        })
        .max_by_key(|s| (s.start_line, std::cmp::Reverse(s.end_line)))
}

fn innermost_any<'a>(syms: &'a [&'a SymRow], line: u32) -> Option<&'a SymRow> {
    syms.iter()
        .copied()
        .filter(|s| s.start_line <= line && line <= s.end_line)
        .max_by_key(|s| (s.start_line, std::cmp::Reverse(s.end_line)))
}

/// Overlay `indexes` onto `g`. See the module docs for the exact semantics.
pub fn apply(g: &mut Graph, indexes: &[ScipIndex], root: &Path) -> ScipStats {
    let mut st = ScipStats {
        indexes: indexes
            .iter()
            .map(|i| crate::sanitize(&i.path.display().to_string()))
            .collect(),
        tools: indexes
            .iter()
            .filter(|i| !i.tool.is_empty())
            .map(|i| crate::sanitize(&i.tool))
            .collect(),
        ..Default::default()
    };
    let file_id: HashMap<&str, i64> = g.files.iter().map(|f| (f.path.as_str(), f.id)).collect();
    let mut by_file: HashMap<i64, Vec<&SymRow>> = HashMap::new();
    for s in &g.symbols {
        by_file.entry(s.file_id).or_default().push(s);
    }

    // 1. which documents are usable
    struct Use<'a> {
        fid: i64,
        doc: &'a Doc,
    }
    let mut usable: Vec<Use> = vec![];
    for idx in indexes {
        for d in &idx.docs {
            st.documents += 1;
            let Some(&fid) = file_id.get(d.path.as_str()) else {
                st.documents_unmatched += 1;
                continue;
            };
            let stale = match (
                idx.mtime,
                std::fs::metadata(root.join(&d.path)).and_then(|m| m.modified()),
            ) {
                (Some(i), Ok(f)) => f > i,
                _ => false,
            };
            if stale {
                st.stale_files += 1;
                continue;
            }
            st.documents_matched += 1;
            usable.push(Use { fid, doc: d });
        }
    }
    let covered: HashSet<i64> = usable.iter().map(|u| u.fid).collect();

    // 2. definitions: scip symbol -> [(document file id, graph symbol id)]. The same symbol string
    //    can be defined in several documents (rust-analyzer: same-named items in different
    //    binary targets), so a reference binds to the definition in its own document, else to
    //    the only definition, else (ambiguous) stays unresolved.
    let mut def_of: HashMap<&str, Vec<(i64, i64)>> = HashMap::new();
    for u in &usable {
        let Some(syms) = by_file.get(&u.fid) else {
            continue;
        };
        for o in u.doc.occs.iter().filter(|o| o.is_def) {
            let Some(name) = symbol_name(&o.symbol) else {
                continue;
            };
            if let Some(s) = innermost(syms, o.line, Some(&name)) {
                let v = def_of.entry(o.symbol.as_str()).or_default();
                if !v.iter().any(|&(f, id)| f == u.fid && id == s.id) {
                    v.push((u.fid, s.id));
                }
            }
        }
    }
    let lookup = |sym: &str, fid: i64| -> Option<i64> {
        let v = def_of.get(sym)?;
        if let Some(&(_, id)) = v.iter().find(|&&(f, _)| f == fid) {
            return Some(id);
        }
        if v.len() == 1 {
            return Some(v[0].1);
        }
        None
    };
    let mapped: HashSet<i64> = def_of.values().flatten().map(|&(_, id)| id).collect();
    st.symbols_mapped = mapped.len();

    // 3. per-occurrence replacement of name-based edges. An occurrence is "resolved" when its
    //    symbol maps to a project symbol, or is defined nowhere in any loaded index (external
    //    library symbol). Symbols defined in the index but not mapped keep name-based edges.
    let defined_anywhere: HashSet<&str> = indexes
        .iter()
        .flat_map(|i| i.docs.iter())
        .flat_map(|d| d.occs.iter())
        .filter(|o| o.is_def)
        .map(|o| o.symbol.as_str())
        .collect();
    let mut resolved: HashSet<(i64, u32, String)> = HashSet::new();
    for u in &usable {
        for o in u.doc.occs.iter().filter(|o| !o.is_def) {
            let Some(name) = symbol_name(&o.symbol) else {
                continue;
            };
            if lookup(&o.symbol, u.fid).is_some() || !defined_anywhere.contains(o.symbol.as_str()) {
                resolved.insert((u.fid, o.line, name));
            }
        }
    }
    let before = g.edges.len();
    g.edges.retain(|e| {
        !(covered.contains(&e.src_file)
            && resolved.contains(&(e.src_file, e.line, e.dst_name.clone())))
    });
    st.edges_replaced = before - g.edges.len();
    g.scip_resolved_refs = resolved;

    // 4. add SCIP edges
    let mut seen: HashSet<(Option<i64>, i64, u32)> = HashSet::new();
    for u in &usable {
        let syms = by_file.get(&u.fid).map(|v| v.as_slice()).unwrap_or(&[]);
        for o in u.doc.occs.iter().filter(|o| !o.is_def && !o.is_import) {
            let Some(dst) = lookup(&o.symbol, u.fid) else {
                continue;
            };
            let src = innermost_any(syms, o.line).map(|s| s.id);
            if !seen.insert((src, dst, o.line)) {
                continue;
            }
            // an occurrence on a symbol's own definition line is its declaration, not a use
            if src == Some(dst)
                && g.symbols
                    .iter()
                    .any(|s| s.id == dst && s.start_line == o.line)
            {
                continue;
            }
            g.edges.push(EdgeRow {
                src_symbol: src,
                src_file: u.fid,
                dst_symbol: dst,
                dst_name: symbol_name(&o.symbol).unwrap_or_default(),
                kind: "references".into(),
                line: o.line,
                confidence: SCIP_EDGE_CONFIDENCE,
                source: "scip".into(),
            });
            st.edges_added += 1;
        }
    }
    g.scip_symbols = mapped;
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_names_per_indexer() {
        assert_eq!(
            symbol_name("rust-analyzer cargo wraith 0.1.0 maps/Region#is_file_backed().")
                .as_deref(),
            Some("is_file_backed")
        );
        assert_eq!(
            symbol_name("scip-typescript npm pkg 1.0.0 src/`db.js`/ArchiveDatabase#signOut().")
                .as_deref(),
            Some("signOut")
        );
        assert_eq!(
            symbol_name("scip-python python pkg 0.1 `a`/A#").as_deref(),
            Some("A")
        );
        assert_eq!(symbol_name("local 12"), None);
        assert_eq!(symbol_name("garbage"), None);
    }

    #[test]
    fn discover_only_known_locations() {
        let t = tempfile::tempdir().unwrap();
        assert!(discover(t.path()).is_empty());
        std::fs::write(t.path().join("index.scip"), b"").unwrap();
        std::fs::create_dir(t.path().join(".plumbgraph")).unwrap();
        std::fs::write(t.path().join(".plumbgraph/index.scip"), b"").unwrap();
        std::fs::write(t.path().join("other.scip"), b"").unwrap();
        assert_eq!(discover(t.path()).len(), 2);
    }
}
