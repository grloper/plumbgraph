//! Language pack loader.
//!
//! A *pack* is a directory containing `pack.toml` plus tree-sitter query files
//! (`defs.scm`, `imports.scm`). Built-in packs are embedded in the binary; extra
//! packs can be loaded from a directory (`<dir>/<pack-id>/pack.toml`). A pack
//! selects one of the **built-in grammars** by name (`python`, `javascript`,
//! `typescript`, `tsx`, `rust`); loading arbitrary native grammars is out of scope.
//!
//! Query convention: in `defs.scm` a match captures the whole definition as
//! `@def.<kind>` and its name node as `@name`. In `imports.scm` a match captures the
//! whole statement as `@import`, the module specifier as `@module`, and optionally
//! imported names as `@name`.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Query};

/// Raw `pack.toml` contents.
#[derive(Debug, Clone, Deserialize)]
pub struct PackManifest {
    pub id: String,
    pub grammar: String,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub manifests: Vec<String>,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub ref_node_kinds: Vec<String>,
    #[serde(default)]
    pub call_node_kinds: Vec<String>,
    #[serde(default)]
    pub string_node_kinds: Vec<String>,
    #[serde(default)]
    pub dynamic_markers: Vec<String>,
    #[serde(default)]
    pub entry_names: Vec<String>,
    #[serde(default)]
    pub test_globs: Vec<String>,
    pub queries: QueryFiles,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QueryFiles {
    pub defs: String,
    pub imports: String,
}

/// A loaded, validated language pack.
#[derive(Debug, Clone)]
pub struct Pack {
    pub manifest: PackManifest,
    pub defs_query: String,
    pub imports_query: String,
    pub builtin: bool,
    /// Queries are compiled once per pack, on first use (compiling per file dominated indexing).
    compiled: std::sync::Arc<Compiled>,
}

#[derive(Debug, Default)]
struct Compiled {
    defs: std::sync::OnceLock<std::result::Result<Query, String>>,
    imports: std::sync::OnceLock<std::result::Result<Query, String>>,
}

/// The language "family" drives the small amount of per-language Rust logic
/// (visibility, import classification). It is derived from the grammar name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Python,
    JsLike,
    Rust,
    /// Go, Java, C#: classes/methods/types with modifier- or case-based visibility
    ClassLike,
}

impl Pack {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn family(&self) -> Family {
        match self.manifest.grammar.as_str() {
            "python" => Family::Python,
            "rust" => Family::Rust,
            "go" | "java" | "c-sharp" => Family::ClassLike,
            _ => Family::JsLike,
        }
    }

    pub fn language(&self) -> Result<Language> {
        grammar(&self.manifest.grammar)
    }

    pub fn defs(&self) -> Result<&Query> {
        self.compiled
            .defs
            .get_or_init(|| {
                let lang = self.language().map_err(|e| e.to_string())?;
                Query::new(&lang, &self.defs_query)
                    .map_err(|e| format!("pack {}: bad defs query: {e}", self.id()))
            })
            .as_ref()
            .map_err(|e| anyhow!("{e}"))
    }

    pub fn imports(&self) -> Result<&Query> {
        self.compiled
            .imports
            .get_or_init(|| {
                let lang = self.language().map_err(|e| e.to_string())?;
                Query::new(&lang, &self.imports_query)
                    .map_err(|e| format!("pack {}: bad imports query: {e}", self.id()))
            })
            .as_ref()
            .map_err(|e| anyhow!("{e}"))
    }

    /// Validate that both queries compile against the grammar.
    pub fn validate(&self) -> Result<()> {
        self.defs()?;
        self.imports()?;
        if self.manifest.extensions.is_empty() {
            bail!("pack {}: no extensions", self.id());
        }
        Ok(())
    }
}

/// Resolve a built-in grammar by name.
pub fn grammar(name: &str) -> Result<Language> {
    Ok(match name {
        "python" => tree_sitter_python::LANGUAGE.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "c-sharp" => tree_sitter_c_sharp::LANGUAGE.into(),
        other => bail!("unknown built-in grammar `{other}`"),
    })
}

/// A set of packs with extension lookup.
#[derive(Debug, Clone, Default)]
pub struct PackSet {
    packs: Vec<Pack>,
}

macro_rules! builtin {
    ($dir:literal) => {
        (
            include_str!(concat!("../packs/", $dir, "/pack.toml")),
            include_str!(concat!("../packs/", $dir, "/defs.scm")),
            include_str!(concat!("../packs/", $dir, "/imports.scm")),
        )
    };
}

impl PackSet {
    /// The packs shipped in the binary: python, javascript, typescript, tsx, rust, go, java, csharp.
    pub fn builtin() -> Result<Self> {
        let sources = [
            builtin!("python"),
            builtin!("javascript"),
            builtin!("typescript"),
            builtin!("tsx"),
            builtin!("rust"),
            builtin!("go"),
            builtin!("java"),
            builtin!("csharp"),
        ];
        let mut packs = Vec::new();
        for (toml_src, defs, imports) in sources {
            let manifest: PackManifest =
                toml::from_str(toml_src).context("parsing built-in pack.toml")?;
            let pack = Pack {
                manifest,
                defs_query: defs.to_string(),
                imports_query: imports.to_string(),
                builtin: true,
                compiled: Default::default(),
            };
            // Built-in packs are validated by the `builtin_packs_load_and_validate` test;
            // compiling every query at startup cost ~100 ms per invocation.
            packs.push(pack);
        }
        Ok(Self { packs })
    }

    /// Load one pack directory (`pack.toml` + query files next to it).
    pub fn load_pack_dir(dir: &Path) -> Result<Pack> {
        let toml_src = std::fs::read_to_string(dir.join("pack.toml"))
            .with_context(|| format!("reading {}/pack.toml", dir.display()))?;
        let manifest: PackManifest = toml::from_str(&toml_src)
            .with_context(|| format!("parsing {}/pack.toml", dir.display()))?;
        // Query paths must stay inside the pack directory.
        for rel in [&manifest.queries.defs, &manifest.queries.imports] {
            let p = Path::new(rel);
            if p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                bail!(
                    "pack {}: query path `{rel}` escapes the pack directory",
                    manifest.id
                );
            }
        }
        let defs_query = std::fs::read_to_string(dir.join(&manifest.queries.defs))?;
        let imports_query = std::fs::read_to_string(dir.join(&manifest.queries.imports))?;
        let pack = Pack {
            manifest,
            defs_query,
            imports_query,
            builtin: false,
            compiled: Default::default(),
        };
        pack.validate()?;
        Ok(pack)
    }

    /// Load every `<dir>/*/pack.toml`. Packs with an id that already exists replace
    /// the existing one (user override).
    pub fn with_dir(mut self, dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            return Ok(self);
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("pack.toml").is_file())
            .collect();
        entries.sort();
        for p in entries {
            let pack = Self::load_pack_dir(&p)?;
            self.packs.retain(|x| x.id() != pack.id());
            self.packs.push(pack);
        }
        Ok(self)
    }

    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }

    pub fn by_id(&self, id: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.id() == id)
    }

    /// Find the pack responsible for a file path by extension (case-insensitive).
    pub fn for_path(&self, path: &Path) -> Option<&Pack> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        self.packs.iter().find(|p| {
            p.manifest
                .extensions
                .iter()
                .any(|e| name.ends_with(&e.to_ascii_lowercase()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_packs_load_and_validate() {
        let set = PackSet::builtin().unwrap();
        let ids: Vec<_> = set.packs().iter().map(|p| p.id().to_string()).collect();
        for want in [
            "python",
            "javascript",
            "typescript",
            "tsx",
            "rust",
            "go",
            "java",
            "csharp",
        ] {
            assert!(ids.contains(&want.to_string()), "missing {want}");
        }
        // built-ins are no longer validated at startup: every one must compile here
        for p in set.packs() {
            p.validate().unwrap();
        }
    }

    #[test]
    fn extension_lookup() {
        let set = PackSet::builtin().unwrap();
        assert_eq!(set.for_path(Path::new("a/b.py")).unwrap().id(), "python");
        assert_eq!(
            set.for_path(Path::new("a/b.MJS")).unwrap().id(),
            "javascript"
        );
        assert_eq!(
            set.for_path(Path::new("a/b.ts")).unwrap().id(),
            "typescript"
        );
        assert_eq!(set.for_path(Path::new("a/b.tsx")).unwrap().id(), "tsx");
        assert_eq!(set.for_path(Path::new("src/lib.rs")).unwrap().id(), "rust");
        assert!(set.for_path(Path::new("README.md")).is_none());
    }

    #[test]
    fn user_pack_dir_overrides_and_rejects_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("py2");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("pack.toml"),
            "id=\"python\"\ngrammar=\"python\"\nextensions=[\".pyx\"]\nqueries={defs=\"d.scm\",imports=\"i.scm\"}\n",
        )
        .unwrap();
        std::fs::write(
            d.join("d.scm"),
            "(function_definition name: (identifier) @name) @def.function\n",
        )
        .unwrap();
        std::fs::write(
            d.join("i.scm"),
            "(import_statement name: (dotted_name) @module) @import\n",
        )
        .unwrap();
        let set = PackSet::builtin().unwrap().with_dir(tmp.path()).unwrap();
        assert!(set.for_path(Path::new("x.pyx")).is_some());
        assert!(!set.by_id("python").unwrap().builtin);

        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(
            bad.join("pack.toml"),
            "id=\"bad\"\ngrammar=\"python\"\nextensions=[\".b\"]\nqueries={defs=\"../d.scm\",imports=\"i.scm\"}\n",
        )
        .unwrap();
        assert!(PackSet::load_pack_dir(&bad).is_err());
    }

    #[test]
    fn invalid_query_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("p");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("pack.toml"), "id=\"p\"\ngrammar=\"python\"\nextensions=[\".p\"]\nqueries={defs=\"d.scm\",imports=\"i.scm\"}\n").unwrap();
        std::fs::write(d.join("d.scm"), "(not_a_real_node) @def.function").unwrap();
        std::fs::write(d.join("i.scm"), "").unwrap();
        assert!(PackSet::load_pack_dir(&d).is_err());
    }
}
