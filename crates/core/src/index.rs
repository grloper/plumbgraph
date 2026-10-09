//! Project indexing: walk, hash, (re)extract changed files, resolve imports, build edges.

use crate::extract::{extract, MAX_FILE_BYTES};
use crate::manifest;
use crate::model::FileFacts;
use crate::resolve::{normalize_pkg, resolve_import, ProjectView};
use crate::store::{Graph, Store, EXTRACTOR_VERSION};
use anyhow::{bail, Context, Result};
use ignore::WalkBuilder;
use plumbgraph_langs::PackSet;
use rayon::prelude::*;
use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "__pycache__",
    "venv",
    ".venv",
    "site-packages",
    ".git",
    ".plumbgraph",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".next",
    "coverage",
];

pub struct IndexOptions {
    pub packs: PackSet,
    /// Database location; default `<root>/.plumbgraph/index.db`.
    pub db_path: Option<PathBuf>,
    /// Re-extract every file even if unchanged.
    pub force: bool,
}

impl IndexOptions {
    pub fn new(packs: PackSet) -> Self {
        Self {
            packs,
            db_path: None,
            force: false,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct IndexStats {
    pub files_seen: usize,
    /// html/json/toml/yaml/cfg/ini files scanned only for name mentions
    pub text_files: usize,
    pub files_parsed: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
    /// files whose bytes were read and hashed (unchanged size+mtime files are not)
    pub files_hashed: usize,
    /// false when nothing changed and import resolution + edges were reused
    pub resolved: bool,
    pub files_skipped_large: usize,
    pub parse_errors: usize,
    pub files_with_syntax_errors: usize,
    pub symbols: usize,
    pub refs: usize,
    pub imports: usize,
    pub edges: usize,
    pub by_language: std::collections::BTreeMap<String, usize>,
    pub db_path: String,
}

const TEXT_MAX_BYTES: usize = 512 * 1024;

/// Files that influence import classification without being indexed themselves.
const MANIFEST_NAMES: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "setup.cfg",
    "setup.py",
    "Pipfile",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];

/// A file modified this recently when indexed may change again without changing its mtime
/// (coarse timestamps): its mtime is recorded as 0 so the next run re-hashes it.
const RACY_NS: i64 = 2_000_000_000;

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn mtime_ns(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Non-code files scanned for identifier mentions (evidence that a name is used from outside indexed code).
fn text_kind(p: &Path) -> Option<&'static str> {
    let name = p.file_name()?.to_str()?.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "cargo.lock"
            | "composer.lock"
            | "plumb-baseline.json"
    ) || name.ends_with(".min.js")
        || name.ends_with(".lock")
    {
        return None;
    }
    match name.rsplit('.').next()? {
        "html" | "htm" => Some("html"),
        "json" | "toml" | "yaml" | "yml" | "cfg" | "ini" => Some("text"),
        _ => None,
    }
}

fn text_tokens(text: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut std::collections::BTreeSet<String>| {
        if cur.len() >= 3 && cur.len() <= 64 && !cur.starts_with(|c: char| c.is_ascii_digit()) {
            out.insert(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '$' {
            cur.push(c);
        } else {
            flush(&mut cur, &mut out);
        }
        if out.len() > 50_000 {
            break;
        }
    }
    flush(&mut cur, &mut out);
    out
}

pub fn db_path_for(root: &Path, opts: &IndexOptions) -> PathBuf {
    opts.db_path
        .clone()
        .unwrap_or_else(|| root.join(".plumbgraph").join("index.db"))
}

/// Configured walker: honours .gitignore, skips hidden and vendor/build directories, does not follow symlinks.
pub fn walker(root: &Path) -> WalkBuilder {
    let mut b = WalkBuilder::new(root);
    b.hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false);
    b.filter_entry(|e| {
        if e.depth() == 0 {
            return true;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        !(is_dir
            && e.file_name()
                .to_str()
                .map(|n| SKIP_DIRS.contains(&n))
                .unwrap_or(false))
    });
    b
}

pub fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn index_project(root: &Path, opts: &IndexOptions) -> Result<IndexStats> {
    let root = root
        .canonicalize()
        .with_context(|| format!("project path {} does not exist", root.display()))?;
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let db_path = db_path_for(&root, opts);
    let store = Store::open(&db_path)?;
    let mut stats = IndexStats {
        db_path: db_path.display().to_string(),
        ..Default::default()
    };

    // 1. walk
    struct Entry {
        rel: String,
        abs: PathBuf,
        pack: String,
    }
    let mut entries: Vec<Entry> = vec![];
    let mut manifest_files: Vec<PathBuf> = vec![];
    for dent in walker(&root).build() {
        let Ok(dent) = dent else { continue };
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(n) = dent.path().file_name().and_then(|n| n.to_str()) {
            if MANIFEST_NAMES.contains(&n) || n.ends_with(".csproj") {
                manifest_files.push(dent.path().to_path_buf());
            }
        }
        if let Some(pack) = opts.packs.for_path(dent.path()) {
            entries.push(Entry {
                rel: rel_path(&root, dent.path()),
                abs: dent.path().to_path_buf(),
                pack: pack.id().to_string(),
            });
        } else if let Some(kind) = text_kind(dent.path()) {
            let big = dent
                .metadata()
                .map(|m| m.len() > TEXT_MAX_BYTES as u64)
                .unwrap_or(true);
            if !big {
                entries.push(Entry {
                    rel: rel_path(&root, dent.path()),
                    abs: dent.path().to_path_buf(),
                    pack: format!("text:{kind}"),
                });
            }
        }
    }
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    stats.text_files = entries
        .iter()
        .filter(|e| e.pack.starts_with("text:"))
        .count();
    stats.files_seen = entries.len() - stats.text_files;

    // 2. hash & diff
    let existing = store.file_stats()?;
    struct Loaded {
        rel: String,
        pack: String,
        hash: String,
        size: usize,
        mtime: i64,
        hashed: bool,
        text: Option<String>,
    }
    let force = opts.force;
    let loaded: Vec<Loaded> = entries
        .par_iter()
        .map(|e| {
            let md = std::fs::metadata(&e.abs).ok();
            let (size, mtime) = md
                .as_ref()
                .map(|m| (m.len() as i64, mtime_ns(m)))
                .unwrap_or((-1, 0));
            if !force {
                if let Some((_, h, s, m)) = existing.get(&e.rel) {
                    if *m != 0 && *m == mtime && *s == size && size >= 0 {
                        return Loaded {
                            rel: e.rel.clone(),
                            pack: e.pack.clone(),
                            hash: h.clone(),
                            size: size as usize,
                            mtime,
                            hashed: false,
                            text: None,
                        };
                    }
                }
            }
            let bytes = std::fs::read(&e.abs).unwrap_or_default();
            let mut h = Sha256::new();
            h.update(EXTRACTOR_VERSION.as_bytes());
            h.update(e.pack.as_bytes());
            h.update(&bytes);
            let hash = format!("{:x}", h.finalize());
            let text = if bytes.len() > MAX_FILE_BYTES {
                None
            } else {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            };
            Loaded {
                rel: e.rel.clone(),
                pack: e.pack.clone(),
                hash,
                size: bytes.len(),
                mtime,
                hashed: true,
                text,
            }
        })
        .collect();
    stats.files_hashed = loaded.iter().filter(|l| l.hashed).count();
    let indexed_at = now_ns();
    let safe_mtime = |m: i64| {
        if m == 0 || indexed_at - m < RACY_NS {
            0
        } else {
            m
        }
    };
    let on_disk: HashSet<&str> = loaded.iter().map(|l| l.rel.as_str()).collect();
    let removed: Vec<i64> = existing
        .iter()
        .filter(|(p, _)| !on_disk.contains(p.as_str()))
        .map(|(_, (id, _, _, _))| *id)
        .collect();
    stats.files_removed = removed.len();
    let to_parse: Vec<&Loaded> = loaded
        .iter()
        .filter(|l| {
            opts.force
                || existing
                    .get(&l.rel)
                    .map(|(_, h, _, _)| h != &l.hash)
                    .unwrap_or(true)
        })
        .collect();
    let text_total = loaded
        .iter()
        .filter(|l| l.pack.starts_with("text:"))
        .count();
    let text_parsed = to_parse
        .iter()
        .filter(|l| l.pack.starts_with("text:"))
        .count();
    stats.files_unchanged = (loaded.len() - text_total) - (to_parse.len() - text_parsed);

    // 3. extract in parallel
    let results: Vec<(&Loaded, std::result::Result<FileFacts, String>)> = to_parse
        .par_iter()
        .map(|l| {
            if let Some(kind) = l.pack.strip_prefix("text:") {
                let facts = FileFacts {
                    lang: kind.to_string(),
                    strings: text_tokens(l.text.as_deref().unwrap_or("")),
                    ..Default::default()
                };
                return (*l, Ok(facts));
            }
            let pack = opts.packs.by_id(&l.pack).expect("pack exists");
            match &l.text {
                None => (*l, Err("skipped: file larger than 2 MiB".to_string())),
                Some(t) => (*l, extract(pack, &l.rel, t).map_err(|e| e.to_string())),
            }
        })
        .collect();

    // 4. write
    store.conn.execute_batch("BEGIN")?;
    let write = (|| -> Result<()> {
        for id in &removed {
            store.delete_file(*id)?;
        }
        for (l, res) in &results {
            if let Some((id, _, _, _)) = existing.get(&l.rel) {
                store.delete_file(*id)?;
            }
            match res {
                Ok(f) => {
                    if f.has_syntax_errors {
                        stats.files_with_syntax_errors += 1;
                    }
                    let lang = if l.pack.starts_with("text:") {
                        f.lang.as_str()
                    } else {
                        l.pack.as_str()
                    };
                    let id = store.insert_file(
                        &l.rel,
                        &l.hash,
                        l.size,
                        Some(f),
                        lang,
                        if f.has_syntax_errors {
                            "syntax-errors"
                        } else {
                            "ok"
                        },
                    )?;
                    store.set_mtime(id, safe_mtime(l.mtime))?;
                    if !l.pack.starts_with("text:") {
                        stats.files_parsed += 1;
                    }
                }
                Err(msg) => {
                    if msg.starts_with("skipped") {
                        stats.files_skipped_large += 1;
                    } else {
                        stats.parse_errors += 1;
                    }
                    let id = store.insert_file(
                        &l.rel,
                        &l.hash,
                        l.size,
                        None,
                        &l.pack,
                        &format!("error: {msg}"),
                    )?;
                    store.set_mtime(id, safe_mtime(l.mtime))?;
                }
            }
        }
        // unchanged content but a new/untrusted mtime: remember the current one
        let parsed: HashSet<&str> = results.iter().map(|(l, _)| l.rel.as_str()).collect();
        for l in &loaded {
            if parsed.contains(l.rel.as_str()) {
                continue;
            }
            if let Some((id, _, _, m)) = existing.get(&l.rel) {
                let want = safe_mtime(l.mtime);
                if *m != want {
                    store.set_mtime(*id, want)?;
                }
            }
        }
        // Import resolution and edges depend only on the indexed files and the manifests:
        // reuse them when neither changed.
        let mut h = Sha256::new();
        for m in &manifest_files {
            h.update(rel_path(&root, m).as_bytes());
            h.update(std::fs::read(m).unwrap_or_default());
        }
        let stamp = format!("{:x}", h.finalize());
        let unchanged = removed.is_empty()
            && results.is_empty()
            && store.meta("resolve_stamp").as_deref() == Some(stamp.as_str())
            && store.meta("edges_built").as_deref() == Some("1");
        if !unchanged {
            resolve_imports(&store, &root, &opts.packs)?;
            build_edges(&store)?;
            store.set_meta("resolve_stamp", &stamp)?;
            store.set_meta("edges_built", "1")?;
            stats.resolved = true;
        }
        Ok(())
    })();
    match write {
        Ok(()) => store.conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = store.conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }

    // 5. stats
    let q = |sql: &str| -> usize {
        store
            .conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    };
    stats.symbols = q("SELECT count(*) FROM symbol");
    stats.refs = q("SELECT count(*) FROM ref");
    stats.imports = q("SELECT count(*) FROM import");
    stats.edges = q("SELECT count(*) FROM edge");
    let mut st = store
        .conn
        .prepare("SELECT lang, count(*) FROM file GROUP BY lang")?;
    for r in st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
    })? {
        let (l, c) = r?;
        stats.by_language.insert(l, c);
    }
    Ok(stats)
}

/// Collect the names of local (workspace) packages from all manifests under the root.
pub fn local_package_names(root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    for m in scan_manifests(root) {
        if let Some(n) = m.package_name {
            out.insert(if m.eco == "npm" { n } else { normalize_pkg(&n) });
        }
    }
    out
}

pub fn scan_manifests(root: &Path) -> Vec<manifest::Manifest> {
    let mut out = vec![];
    for dent in walker(root).build().flatten() {
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(name) = dent.file_name().to_str() else {
            continue;
        };
        if manifest::is_manifest_name(name).is_none() {
            continue;
        }
        if dent
            .metadata()
            .map(|m| m.len() > 4 * 1024 * 1024)
            .unwrap_or(true)
        {
            continue;
        }
        let rel = rel_path(root, dent.path());
        if let Some(m) = manifest::read(root, &rel) {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn resolve_imports(store: &Store, root: &Path, packs: &PackSet) -> Result<()> {
    let g = Graph::load(store)?;
    let files: Vec<(String, String)> = g
        .files
        .iter()
        .map(|f| (f.path.clone(), f.lang.clone()))
        .collect();
    let mut rust_mods: HashSet<String> = HashSet::new();
    for s in &g.symbols {
        if s.kind == "module" {
            rust_mods.insert(s.name.clone());
        }
    }
    for i in &g.imports {
        if i.kind == "mod" {
            rust_mods.insert(i.module.clone());
        }
    }
    // local package names: npm names are kept verbatim, others normalized; the resolver checks
    // the form appropriate to each language so insert both.
    let mut local = HashSet::new();
    for n in local_package_names(root) {
        local.insert(n.clone());
        local.insert(normalize_pkg(&n));
    }
    let view = ProjectView::build(&files, rust_mods, local);
    let mut bound: HashMap<i64, HashSet<String>> = HashMap::new();
    for i in &g.imports {
        if i.kind == "import" && g.file(i.file_id).map(|f| f.lang == "rust").unwrap_or(false) {
            bound
                .entry(i.file_id)
                .or_default()
                .extend(crate::resolve::rust_use_bindings(&i.module));
        }
    }
    let no_bound: HashSet<String> = HashSet::new();
    let by_id: HashMap<i64, &crate::store::FileRow> = g.files.iter().map(|f| (f.id, f)).collect();
    let mut st = store
        .conn
        .prepare("SELECT id, file_id, module, names, line, kind FROM import")?;
    let rows: Vec<(i64, i64, String, String, u32, String)> = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    drop(st);
    let mut up = store
        .conn
        .prepare("UPDATE import SET class=?1, resolved_files=?2, package=?3 WHERE id=?4")?;
    for (id, file_id, module, names, line, kind) in rows {
        let Some(f) = by_id.get(&file_id) else {
            continue;
        };
        let Some(pack) = packs.by_id(&f.lang) else {
            continue;
        };
        let imp = crate::model::ImportFact {
            module,
            names: serde_json::from_str(&names).unwrap_or_default(),
            line,
            kind,
        };
        let r = resolve_import(
            &view,
            pack.family(),
            &f.path,
            &imp,
            bound.get(&file_id).unwrap_or(&no_bound),
        );
        up.execute(params![
            r.class,
            serde_json::to_string(&r.resolved_files)?,
            r.package,
            id
        ])?;
    }
    Ok(())
}

fn build_edges(store: &Store) -> Result<()> {
    store.conn.execute("DELETE FROM edge", [])?;
    let g = Graph::load(store)?;
    let path_to_id: HashMap<&str, i64> = g.files.iter().map(|f| (f.path.as_str(), f.id)).collect();
    let mut sym_by_file_idx: HashMap<(i64, i64), i64> = HashMap::new();
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in g.symbols.iter().enumerate() {
        sym_by_file_idx.insert((s.file_id, s.idx), s.id);
        by_name.entry(s.name.as_str()).or_default().push(i);
    }
    let mut imported: HashMap<i64, (HashSet<i64>, HashSet<&str>)> = HashMap::new();
    for imp in &g.imports {
        let e = imported.entry(imp.file_id).or_default();
        for p in &imp.resolved_files {
            if let Some(id) = path_to_id.get(p.as_str()) {
                e.0.insert(*id);
            }
        }
        for n in &imp.names {
            e.1.insert(n.as_str());
        }
    }
    let mut ins = store.conn.prepare("INSERT INTO edge(src_symbol,src_file,dst_symbol,dst_name,kind,line,confidence,source,resolved) VALUES(?1,?2,?3,?4,?5,?6,?7,'t0-treesitter',?8)")?;
    let empty = (HashSet::new(), HashSet::new());
    for r in &g.refs {
        let Some(all) = by_name.get(r.name.as_str()) else {
            continue;
        };
        // `x.name` / `Path::name` may be a method or a module-level function: consider all.
        // A bare `name` cannot be a method; if only methods exist (e.g. inside macro token trees)
        // keep them as weak edges.
        let cands: Vec<usize> = if r.member {
            all.clone()
        } else {
            let non_methods: Vec<usize> = all
                .iter()
                .copied()
                .filter(|&i| g.symbols[i].kind != "method")
                .collect();
            if non_methods.is_empty() {
                all.clone()
            } else {
                non_methods
            }
        };
        if cands.is_empty() {
            continue;
        }
        let only_methods_fallback =
            !r.member && cands.iter().all(|&i| g.symbols[i].kind == "method");
        let (imp_files, imp_names) = imported.get(&r.file_id).unwrap_or(&empty);
        let same: Vec<usize> = cands
            .iter()
            .copied()
            .filter(|&i| g.symbols[i].file_id == r.file_id)
            .collect();
        let (chosen, mut conf, resolved): (Vec<usize>, f64, bool) = if !same.is_empty() {
            (same, 0.8, true)
        } else {
            let via_import: Vec<usize> = cands
                .iter()
                .copied()
                .filter(|&i| imp_files.contains(&g.symbols[i].file_id))
                .collect();
            if !via_import.is_empty() {
                let c = if imp_names.contains(r.name.as_str()) {
                    0.85
                } else {
                    0.7
                };
                (via_import, c, true)
            } else if cands.len() == 1 {
                (cands.clone(), 0.5, false)
            } else {
                (cands.clone(), 0.4, false)
            }
        };
        if r.member {
            conf -= 0.1;
        }
        if only_methods_fallback {
            conf = 0.3; // bare name matching only methods (e.g. inside macro token trees): keep as a weak edge
        }
        let src_symbol = r
            .src_idx
            .and_then(|ix| sym_by_file_idx.get(&(r.file_id, ix)).copied());
        for i in chosen {
            let dst = &g.symbols[i];
            ins.execute(params![
                src_symbol, r.file_id, dst.id, r.name, r.kind, r.line, conf, resolved
            ])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn opts(root: &Path) -> IndexOptions {
        let mut o = IndexOptions::new(PackSet::builtin().unwrap());
        o.db_path = Some(root.join(".plumbgraph/index.db"));
        o
    }

    #[test]
    fn index_incremental_and_edges() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(
            r,
            "a.py",
            "from b import helper\n\ndef run():\n    return helper()\n",
        );
        write(
            r,
            "b.py",
            "def helper():\n    return 1\n\ndef lonely():\n    pass\n",
        );
        write(r, "node_modules/x/index.js", "function junk(){}");
        write(r, ".gitignore", "ignored.py\n");
        write(r, "ignored.py", "def nope(): pass\n");
        let s1 = index_project(r, &opts(r)).unwrap();
        assert_eq!(
            (s1.files_seen, s1.files_parsed, s1.files_unchanged),
            (2, 2, 0)
        );
        assert_eq!(s1.symbols, 3);
        assert!(s1.edges >= 1);
        // import edge helper -> b.py::helper with high-ish confidence
        let g = Graph::load(&Store::open(&r.join(".plumbgraph/index.db")).unwrap()).unwrap();
        let helper = g.symbols.iter().find(|s| s.name == "helper").unwrap();
        let e = g.edges.iter().find(|e| e.dst_symbol == helper.id).unwrap();
        assert!(e.confidence >= 0.85 && e.kind == "calls");
        // unchanged
        let s2 = index_project(r, &opts(r)).unwrap();
        assert_eq!((s2.files_parsed, s2.files_unchanged), (0, 2));
        // change + remove
        write(r, "b.py", "def helper():\n    return 2\n");
        std::fs::remove_file(r.join("a.py")).unwrap();
        let s3 = index_project(r, &opts(r)).unwrap();
        assert_eq!((s3.files_parsed, s3.files_removed, s3.symbols), (1, 1, 1));
    }

    #[test]
    fn symlink_escape_is_not_followed() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.py", "def leaked(): pass\n");
        write(t.path(), "ok.py", "def fine(): pass\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), t.path().join("link")).unwrap();
        let s = index_project(t.path(), &opts(t.path())).unwrap();
        assert_eq!(s.files_seen, 1);
    }

    #[test]
    fn syntax_error_file_is_reported_not_fatal() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "bad.py", "def (:\n  ???\n");
        let s = index_project(t.path(), &opts(t.path())).unwrap();
        assert_eq!(s.files_with_syntax_errors, 1);
    }

    #[test]
    fn unchanged_files_are_not_read_and_edges_are_reused() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(r, "a.py", "def f():\n    return 1\n");
        write(r, "b.py", "from a import f\n\nprint(f())\n");
        let o = opts(r);
        let s1 = index_project(r, &o).unwrap();
        assert_eq!(s1.files_hashed, 2);
        assert!(s1.resolved);
        // racy window: files modified in the last 2 s are re-hashed once more
        let s2 = index_project(r, &o).unwrap();
        assert_eq!(s2.files_parsed, 0);
        // age the files so their mtimes are trusted, then index again
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for f in ["a.py", "b.py"] {
            std::fs::File::options()
                .write(true)
                .open(r.join(f))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let s3 = index_project(r, &o).unwrap();
        assert_eq!(s3.files_parsed, 0);
        let s4 = index_project(r, &o).unwrap();
        assert_eq!(s4.files_hashed, 0, "trusted size+mtime: no file is read");
        assert!(!s4.resolved, "edges reused when nothing changed");
        assert_eq!(s4.edges, s1.edges);
        // same size, same mtime would be missed by design only inside the racy window; a real
        // content change with a new mtime is always found
        write(r, "a.py", "def f():\n    return 2\n");
        let s5 = index_project(r, &o).unwrap();
        assert_eq!((s5.files_hashed, s5.files_parsed), (1, 1));
        assert!(s5.resolved);
    }

    #[test]
    fn same_size_edit_inside_the_racy_window_is_still_detected() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(r, "a.py", "def f():\n    return 1\n");
        let o = opts(r);
        index_project(r, &o).unwrap();
        let m = std::fs::metadata(r.join("a.py"))
            .unwrap()
            .modified()
            .unwrap();
        write(r, "a.py", "def g():\n    return 1\n");
        std::fs::File::options()
            .write(true)
            .open(r.join("a.py"))
            .unwrap()
            .set_modified(m)
            .unwrap();
        let s = index_project(r, &o).unwrap();
        assert_eq!(s.files_parsed, 1, "identical size and mtime, new content");
        let g = Graph::load(&Store::open(&db_path_for(r, &o)).unwrap()).unwrap();
        assert!(g.symbols.iter().any(|x| x.name == "g"));
    }

    #[test]
    fn manifest_change_forces_re_resolution() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(r, "a.py", "import requests\n");
        write(r, "requirements.txt", "requests\n");
        let o = opts(r);
        index_project(r, &o).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(r.join("a.py"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        index_project(r, &o).unwrap();
        assert!(!index_project(r, &o).unwrap().resolved);
        write(r, "requirements.txt", "requests\nflask\n");
        assert!(index_project(r, &o).unwrap().resolved);
    }
}
