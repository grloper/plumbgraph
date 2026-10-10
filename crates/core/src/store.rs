//! SQLite store (WAL) for the code graph.

use crate::model::FileFacts;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;

pub const SCHEMA_VERSION: &str = "2";
/// Bump when extraction logic changes so cached facts are re-extracted.
pub const EXTRACTOR_VERSION: &str = "4";

pub struct Store {
    pub conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS file(
  id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, lang TEXT NOT NULL, hash TEXT NOT NULL, size INTEGER,
  is_test INTEGER NOT NULL DEFAULT 0, is_generated INTEGER NOT NULL DEFAULT 0, parse_status TEXT NOT NULL DEFAULT 'ok',
  dynamic TEXT NOT NULL DEFAULT '[]', mtime_ns INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS symbol(
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE, idx INTEGER NOT NULL,
  kind TEXT NOT NULL, name TEXT NOT NULL, qname TEXT NOT NULL, start_line INTEGER, end_line INTEGER,
  exported INTEGER, is_test INTEGER, entry TEXT, entry_cond TEXT, decorated INTEGER, subclass INTEGER NOT NULL DEFAULT 0, keep TEXT, parent_idx INTEGER);
CREATE INDEX IF NOT EXISTS symbol_name ON symbol(name);
CREATE INDEX IF NOT EXISTS symbol_file ON symbol(file_id);
CREATE TABLE IF NOT EXISTS ref(
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  src_idx INTEGER, name TEXT NOT NULL, kind TEXT NOT NULL, line INTEGER, member INTEGER);
CREATE INDEX IF NOT EXISTS ref_file ON ref(file_id);
CREATE TABLE IF NOT EXISTS import(
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  module TEXT NOT NULL, names TEXT NOT NULL, line INTEGER, kind TEXT, class TEXT, resolved_files TEXT NOT NULL DEFAULT '[]', package TEXT);
CREATE INDEX IF NOT EXISTS import_file ON import(file_id);
CREATE TABLE IF NOT EXISTS strmention(file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE, name TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS strmention_file ON strmention(file_id);
CREATE TABLE IF NOT EXISTS edge(
  src_symbol INTEGER, src_file INTEGER NOT NULL, dst_symbol INTEGER NOT NULL, dst_name TEXT NOT NULL,
  kind TEXT NOT NULL, line INTEGER, confidence REAL NOT NULL, source TEXT NOT NULL, resolved INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS edge_src ON edge(src_symbol);
CREATE INDEX IF NOT EXISTS edge_dst ON edge(dst_symbol);
"#;

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut conn =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key='schema'", [], |r| {
                r.get(0)
            })
            .ok();
        let ext: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key='extractor'", [], |r| {
                r.get(0)
            })
            .ok();
        if version.is_some()
            && (version.as_deref() != Some(SCHEMA_VERSION)
                || ext.as_deref() != Some(EXTRACTOR_VERSION))
        {
            // The DB is a cache: rebuild on any version change.
            drop(conn);
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.as_os_str().to_owned();
                p.push(suffix);
                let _ = std::fs::remove_file(p);
            }
            conn = Connection::open(path)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
        }
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('schema',?1)",
            [SCHEMA_VERSION],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES('extractor',?1)",
            [EXTRACTOR_VERSION],
        )?;
        Ok(Store { conn })
    }

    pub fn file_hashes(&self) -> Result<HashMap<String, (i64, String)>> {
        let mut st = self.conn.prepare("SELECT path, id, hash FROM file")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)?, r.get::<_, String>(2)?),
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// `(id, hash, size, mtime_ns)` per path. `mtime_ns == 0` means "not trustworthy, re-hash".
    pub fn file_stats(&self) -> Result<HashMap<String, (i64, String, i64, i64)>> {
        let mut st = self
            .conn
            .prepare("SELECT path, id, hash, size, mtime_ns FROM file")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?.unwrap_or(-1),
                    r.get::<_, i64>(4)?,
                ),
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn set_mtime(&self, id: i64, mtime_ns: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE file SET mtime_ns=?1 WHERE id=?2",
            params![mtime_ns, id],
        )?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .ok()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_file(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM file WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn insert_file(
        &self,
        path: &str,
        hash: &str,
        size: usize,
        facts: Option<&FileFacts>,
        lang: &str,
        status: &str,
    ) -> Result<i64> {
        let (is_test, is_gen, dynamic) = match facts {
            Some(f) => (
                f.is_test,
                f.is_generated,
                serde_json::to_string(&f.dynamic_markers)?,
            ),
            None => (false, false, "[]".into()),
        };
        self.conn.execute(
            "INSERT INTO file(path,lang,hash,size,is_test,is_generated,parse_status,dynamic) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![path, lang, hash, size as i64, is_test, is_gen, status, dynamic],
        )?;
        let id = self.conn.last_insert_rowid();
        let Some(f) = facts else { return Ok(id) };
        {
            let mut st = self.conn.prepare_cached(
                "INSERT INTO symbol(file_id,idx,kind,name,qname,start_line,end_line,exported,is_test,entry,entry_cond,decorated,subclass,keep,parent_idx) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            )?;
            for (i, s) in f.symbols.iter().enumerate() {
                st.execute(params![
                    id,
                    i as i64,
                    s.kind,
                    s.name,
                    s.qname,
                    s.start_line,
                    s.end_line,
                    s.exported,
                    s.is_test,
                    s.entry,
                    s.entry_cond,
                    s.decorated,
                    s.subclass_method,
                    s.keep,
                    s.parent.map(|p| p as i64)
                ])?;
            }
        }
        {
            let mut st = self.conn.prepare_cached(
                "INSERT INTO ref(file_id,src_idx,name,kind,line,member) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for r in &f.refs {
                st.execute(params![
                    id,
                    r.src.map(|x| x as i64),
                    r.name,
                    r.kind,
                    r.line,
                    r.member
                ])?;
            }
        }
        {
            let mut st = self.conn.prepare_cached(
                "INSERT INTO import(file_id,module,names,line,kind) VALUES(?1,?2,?3,?4,?5)",
            )?;
            for i in &f.imports {
                st.execute(params![
                    id,
                    i.module,
                    serde_json::to_string(&i.names)?,
                    i.line,
                    i.kind
                ])?;
            }
        }
        {
            let mut st = self
                .conn
                .prepare_cached("INSERT INTO strmention(file_id,name) VALUES(?1,?2)")?;
            for s in &f.strings {
                st.execute(params![id, s])?;
            }
        }
        Ok(id)
    }
}

#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub lang: String,
    pub is_test: bool,
    pub is_generated: bool,
    pub parse_status: String,
    pub dynamic: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SymRow {
    pub id: i64,
    pub file_id: i64,
    pub idx: i64,
    pub kind: String,
    pub name: String,
    pub qname: String,
    pub start_line: u32,
    pub end_line: u32,
    pub exported: bool,
    pub is_test: bool,
    pub entry: Option<String>,
    pub entry_cond: Option<String>,
    pub decorated: bool,
    pub subclass: bool,
    pub keep: Option<String>,
    pub parent_idx: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RefRow {
    pub file_id: i64,
    pub src_idx: Option<i64>,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub member: bool,
}

#[derive(Debug, Clone)]
pub struct ImportRow {
    pub file_id: i64,
    pub module: String,
    pub names: Vec<String>,
    pub line: u32,
    pub kind: String,
    pub class: String,
    pub resolved_files: Vec<String>,
    pub package: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EdgeRow {
    pub src_symbol: Option<i64>,
    pub src_file: i64,
    pub dst_symbol: i64,
    pub dst_name: String,
    pub kind: String,
    pub line: u32,
    pub confidence: f64,
    pub source: String,
}

/// The whole graph loaded in memory.
#[derive(Debug, Default)]
pub struct Graph {
    pub files: Vec<FileRow>,
    pub symbols: Vec<SymRow>,
    pub refs: Vec<RefRow>,
    pub imports: Vec<ImportRow>,
    pub edges: Vec<EdgeRow>,
    /// names mentioned in string literals, per file id
    pub strings: HashMap<i64, Vec<String>>,
    /// Symbol ids whose references were resolved from a SCIP index (see `scip::apply`).
    pub scip_symbols: std::collections::HashSet<i64>,
    /// `(file id, line, name)` occurrences a SCIP index resolved; their name-based edges were replaced.
    pub scip_resolved_refs: std::collections::HashSet<(i64, u32, String)>,
}

impl Graph {
    pub fn load(store: &Store) -> Result<Graph> {
        let c = &store.conn;
        let mut g = Graph::default();
        let mut st = c.prepare(
            "SELECT id,path,lang,is_test,is_generated,parse_status,dynamic FROM file ORDER BY path",
        )?;
        for r in st.query_map([], |r| {
            Ok(FileRow {
                id: r.get(0)?,
                path: r.get(1)?,
                lang: r.get(2)?,
                is_test: r.get(3)?,
                is_generated: r.get(4)?,
                parse_status: r.get(5)?,
                dynamic: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
            })
        })? {
            g.files.push(r?);
        }
        let mut st = c.prepare("SELECT id,file_id,idx,kind,name,qname,start_line,end_line,exported,is_test,entry,entry_cond,decorated,subclass,keep,parent_idx FROM symbol ORDER BY file_id, idx")?;
        for r in st.query_map([], |r| {
            Ok(SymRow {
                id: r.get(0)?,
                file_id: r.get(1)?,
                idx: r.get(2)?,
                kind: r.get(3)?,
                name: r.get(4)?,
                qname: r.get(5)?,
                start_line: r.get(6)?,
                end_line: r.get(7)?,
                exported: r.get(8)?,
                is_test: r.get(9)?,
                entry: r.get(10)?,
                entry_cond: r.get(11)?,
                decorated: r.get(12)?,
                subclass: r.get(13)?,
                keep: r.get(14)?,
                parent_idx: r.get(15)?,
            })
        })? {
            g.symbols.push(r?);
        }
        let mut st =
            c.prepare("SELECT file_id,src_idx,name,kind,line,member FROM ref ORDER BY id")?;
        for r in st.query_map([], |r| {
            Ok(RefRow {
                file_id: r.get(0)?,
                src_idx: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                line: r.get(4)?,
                member: r.get(5)?,
            })
        })? {
            g.refs.push(r?);
        }
        let mut st = c.prepare("SELECT file_id,module,names,line,kind,class,resolved_files,package FROM import ORDER BY id")?;
        for r in st.query_map([], |r| {
            Ok(ImportRow {
                file_id: r.get(0)?,
                module: r.get(1)?,
                names: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                line: r.get(3)?,
                kind: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                class: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                resolved_files: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                package: r.get(7)?,
            })
        })? {
            g.imports.push(r?);
        }
        let mut st = c.prepare(
            "SELECT src_symbol,src_file,dst_symbol,dst_name,kind,line,confidence,source FROM edge",
        )?;
        for r in st.query_map([], |r| {
            Ok(EdgeRow {
                src_symbol: r.get(0)?,
                src_file: r.get(1)?,
                dst_symbol: r.get(2)?,
                dst_name: r.get(3)?,
                kind: r.get(4)?,
                line: r.get(5)?,
                confidence: r.get(6)?,
                source: r.get(7)?,
            })
        })? {
            g.edges.push(r?);
        }
        let mut st = c.prepare("SELECT file_id,name FROM strmention")?;
        for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (f, n) = r?;
            g.strings.entry(f).or_default().push(n);
        }
        Ok(g)
    }

    pub fn file(&self, id: i64) -> Option<&FileRow> {
        self.files.iter().find(|f| f.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn roundtrip_and_cascade() {
        let tmp = tempfile::tempdir().unwrap();
        let st = Store::open(&tmp.path().join("i.db")).unwrap();
        let mut f = FileFacts {
            lang: "python".into(),
            ..Default::default()
        };
        f.symbols.push(SymbolFact {
            name: "a".into(),
            qname: "a".into(),
            kind: "function".into(),
            start_line: 1,
            end_line: 2,
            start_byte: 0,
            end_byte: 5,
            exported: true,
            is_test: false,
            entry: None,
            entry_cond: None,
            decorated: false,
            subclass_method: false,
            keep: None,
            parent: None,
        });
        f.refs.push(RefFact {
            name: "a".into(),
            kind: "calls".into(),
            line: 3,
            member: false,
            src: None,
        });
        f.imports.push(ImportFact {
            module: "os".into(),
            names: vec![],
            line: 1,
            kind: "import".into(),
        });
        f.strings.insert("a".into());
        let id = st
            .insert_file("a.py", "h", 10, Some(&f), "python", "ok")
            .unwrap();
        let g = Graph::load(&st).unwrap();
        assert_eq!(
            (
                g.files.len(),
                g.symbols.len(),
                g.refs.len(),
                g.imports.len()
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(g.strings[&id], vec!["a".to_string()]);
        st.delete_file(id).unwrap();
        let g = Graph::load(&st).unwrap();
        assert_eq!(
            (
                g.files.len(),
                g.symbols.len(),
                g.refs.len(),
                g.imports.len()
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn version_mismatch_rebuilds() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("i.db");
        {
            let st = Store::open(&p).unwrap();
            st.conn
                .execute("UPDATE meta SET value='0' WHERE key='schema'", [])
                .unwrap();
            st.insert_file("a.py", "h", 1, None, "python", "ok")
                .unwrap();
        }
        let st = Store::open(&p).unwrap();
        assert!(st.file_hashes().unwrap().is_empty());
    }
}
