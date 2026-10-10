//! Import classification and best-effort local resolution (relative/workspace/stdlib/external).
//!
//! This is *heuristic* resolution: it only looks at file names and manifests that were
//! indexed. It never executes project code or package managers.

use crate::model::ImportFact;
use plumbgraph_langs::Family;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// `local` | `stdlib` | `external` | `unresolved` (relative import that matches no file) | `alias` (tsconfig-style alias, not resolved) | `url`
    pub class: String,
    pub resolved_files: Vec<String>,
    /// For `external`: the package name as written in source (before manifest/registry mapping).
    pub package: Option<String>,
}

impl Resolution {
    fn new(class: &str) -> Self {
        Resolution {
            class: class.into(),
            resolved_files: vec![],
            package: None,
        }
    }
}

/// What the resolver knows about the project.
#[derive(Debug, Default)]
pub struct ProjectView {
    pub paths: HashSet<String>,
    /// dotted-suffix -> python file (module or package `__init__`)
    pub py_modules: HashMap<String, String>,
    /// dotted-suffix of directories (namespace packages)
    pub py_dirs: HashSet<String>,
    pub rust_files: Vec<String>,
    pub rust_mods: HashSet<String>,
    /// package names that are local (workspace members): npm names, cargo package names (normalized `_`), python project names
    pub local_pkgs: HashSet<String>,
}

impl ProjectView {
    /// Build from `(path, lang)` pairs, the set of module names declared in Rust, and local package names.
    pub fn build(
        files: &[(String, String)],
        rust_mods: HashSet<String>,
        local_pkgs: HashSet<String>,
    ) -> Self {
        let mut v = ProjectView {
            rust_mods,
            local_pkgs,
            ..Default::default()
        };
        for (p, lang) in files {
            v.paths.insert(p.clone());
            if lang == "python" {
                let no_ext = p.trim_end_matches(".pyi").trim_end_matches(".py");
                let parts: Vec<&str> = no_ext.split('/').collect();
                let (mod_parts, dir_parts): (Vec<&str>, Vec<&str>) =
                    if parts.last() == Some(&"__init__") {
                        (
                            parts[..parts.len() - 1].to_vec(),
                            parts[..parts.len() - 1].to_vec(),
                        )
                    } else {
                        (parts.clone(), parts[..parts.len() - 1].to_vec())
                    };
                for i in 0..mod_parts.len() {
                    let key = mod_parts[i..].join(".");
                    if !key.is_empty() {
                        v.py_modules.entry(key).or_insert_with(|| p.clone());
                    }
                }
                for i in 0..dir_parts.len() {
                    let key = dir_parts[i..].join(".");
                    if !key.is_empty() {
                        v.py_dirs.insert(key);
                    }
                }
            } else if lang == "rust" {
                v.rust_files.push(p.clone());
            }
        }
        v.rust_files.sort();
        v
    }
}

pub const PY_STDLIB: &[&str] = &[
    "__future__",
    "_thread",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "base64",
    "bdb",
    "binascii",
    "bisect",
    "builtins",
    "bz2",
    "calendar",
    "cgi",
    "cmath",
    "cmd",
    "code",
    "codecs",
    "collections",
    "colorsys",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "cProfile",
    "csv",
    "ctypes",
    "curses",
    "dataclasses",
    "datetime",
    "dbm",
    "decimal",
    "difflib",
    "dis",
    "doctest",
    "email",
    "encodings",
    "enum",
    "errno",
    "faulthandler",
    "fcntl",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getopt",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "grp",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "imaplib",
    "imghdr",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "lib2to3",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "mailbox",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "modulefinder",
    "msvcrt",
    "multiprocessing",
    "netrc",
    "numbers",
    "operator",
    "optparse",
    "os",
    "pathlib",
    "pdb",
    "pickle",
    "pickletools",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posix",
    "posixpath",
    "pprint",
    "profile",
    "pstats",
    "pty",
    "pwd",
    "py_compile",
    "pyclbr",
    "pydoc",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "rlcompleter",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtplib",
    "sndhdr",
    "socket",
    "socketserver",
    "sqlite3",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "symtable",
    "sys",
    "sysconfig",
    "syslog",
    "tabnanny",
    "tarfile",
    "telnetlib",
    "tempfile",
    "termios",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "turtle",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uuid",
    "venv",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "winreg",
    "winsound",
    "wsgiref",
    "xdrlib",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
    "_typeshed",
];

pub const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
    "test",
    "sqlite",
];

/// Names bound by a Rust `use` argument: `a::b::{c, d::e, f as g, self}` -> c, e, g, b.
pub fn rust_use_bindings(arg: &str) -> Vec<String> {
    fn split_top(s: &str) -> Vec<&str> {
        let (mut depth, mut start, mut out) = (0i32, 0usize, vec![]);
        for (i, c) in s.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(s[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        out.push(s[start..].trim());
        out.into_iter().filter(|x| !x.is_empty()).collect()
    }
    fn go(s: &str, out: &mut Vec<String>) {
        let s = s.trim();
        if let (Some(open), Some(close)) = (s.find('{'), s.rfind('}')) {
            let prefix = s[..open].trim_end_matches("::").trim();
            let last = prefix.rsplit("::").next().unwrap_or("").trim();
            for part in split_top(&s[open + 1..close]) {
                if part == "self" {
                    if !last.is_empty() {
                        out.push(last.to_string());
                    }
                } else {
                    go(part, out);
                }
            }
        } else if let Some((_, alias)) = s.rsplit_once(" as ") {
            out.push(alias.trim().to_string());
        } else if !s.ends_with('*') {
            if let Some(l) = s.rsplit("::").next() {
                if !l.is_empty() {
                    out.push(l.to_string());
                }
            }
        }
    }
    let mut out = vec![];
    go(arg, &mut out);
    out
}

pub const RUST_STD: &[&str] = &["std", "core", "alloc", "proc_macro", "test"];

pub fn normalize_pkg(name: &str) -> String {
    name.to_ascii_lowercase().replace(['_', '.'], "-")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn join_norm(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base.is_empty() {
        vec![]
    } else {
        base.split('/').collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

pub fn resolve_import(
    view: &ProjectView,
    family: Family,
    file_path: &str,
    imp: &ImportFact,
    bound: &HashSet<String>,
) -> Resolution {
    match family {
        Family::Python => resolve_python(view, file_path, imp),
        Family::JsLike => resolve_js(view, file_path, imp),
        Family::Rust => resolve_rust(view, file_path, imp, bound),
        Family::ClassLike => resolve_classlike(view, file_path, imp),
    }
}

/// Go (package path -> directory), Java (qualified name -> file/dir). C# `using` is namespace
/// based and namespaces are not indexed, so non-System namespaces stay `unresolved`.
fn resolve_classlike(view: &ProjectView, file_path: &str, imp: &ImportFact) -> Resolution {
    let m = imp.module.as_str();
    let ext = file_path.rsplit('.').next().unwrap_or("");
    let files_in_dir = |dir: &str, ext: &str| -> Vec<String> {
        let mut v: Vec<String> = view
            .paths
            .iter()
            .filter(|p| p.ends_with(ext) && dir_of(p) == dir && !p.ends_with("_test.go"))
            .cloned()
            .collect();
        v.sort();
        v
    };
    match ext {
        "go" => {
            let mut dirs: Vec<&str> = view
                .paths
                .iter()
                .filter(|p| p.ends_with(".go"))
                .map(|p| dir_of(p))
                .filter(|d| !d.is_empty() && (m == *d || m.ends_with(&format!("/{d}"))))
                .collect();
            dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
            if let Some(d) = dirs.first() {
                let mut r = Resolution::new("local");
                r.resolved_files = files_in_dir(d, ".go");
                return r;
            }
            if !m.split('/').next().unwrap_or("").contains('.') {
                return Resolution::new("stdlib");
            }
            let mut r = Resolution::new("external");
            r.package = Some(m.to_string());
            r
        }
        "java" => {
            let rel = format!("{}.java", m.replace('.', "/"));
            let mut hit: Vec<String> = view
                .paths
                .iter()
                .filter(|p| **p == rel || p.ends_with(&format!("/{rel}")))
                .cloned()
                .collect();
            if hit.is_empty() {
                // wildcard / package import: every file in a directory ending with the package path
                let pkg = m.replace('.', "/");
                let mut dirs: Vec<&str> = view
                    .paths
                    .iter()
                    .filter(|p| p.ends_with(".java"))
                    .map(|p| dir_of(p))
                    .filter(|d| *d == pkg || d.ends_with(&format!("/{pkg}")))
                    .collect();
                dirs.sort();
                dirs.dedup();
                for d in dirs {
                    hit.extend(files_in_dir(d, ".java"));
                }
            }
            if !hit.is_empty() {
                hit.sort();
                let mut r = Resolution::new("local");
                r.resolved_files = hit;
                return r;
            }
            if ["java.", "javax.", "jdk.", "sun."]
                .iter()
                .any(|p| m.starts_with(p))
            {
                return Resolution::new("stdlib");
            }
            Resolution::new("external")
        }
        _ => {
            if m == "System" || m.starts_with("System.") || m.starts_with("Microsoft.") {
                Resolution::new("stdlib")
            } else {
                // A C# `using` names a namespace, which cannot be mapped to a file without a
                // compiler (many files share one namespace; NuGet packages and test frameworks
                // look the same). Not "unresolved": there is no evidence it is missing.
                Resolution::new("external")
            }
        }
    }
}

fn resolve_python(view: &ProjectView, file_path: &str, imp: &ImportFact) -> Resolution {
    let m = imp.module.as_str();
    if m.starts_with('.') {
        let dots = m.chars().take_while(|c| *c == '.').count();
        let rest = &m[dots..];
        let mut base = dir_of(file_path).to_string();
        for _ in 1..dots {
            base = dir_of(&base).to_string();
        }
        let mut files = vec![];
        let rel = rest.replace('.', "/");
        let push_cands = |stem: &str, files: &mut Vec<String>| {
            for c in [
                format!("{stem}.py"),
                format!("{stem}.pyi"),
                format!("{stem}/__init__.py"),
            ] {
                if view.paths.contains(&c) {
                    files.push(c);
                    return true;
                }
            }
            false
        };
        let base_join = |r: &str| {
            if base.is_empty() {
                r.to_string()
            } else if r.is_empty() {
                base.clone()
            } else {
                format!("{base}/{r}")
            }
        };
        let mut found = false;
        if !rel.is_empty() {
            found = push_cands(&base_join(&rel), &mut files);
        } else {
            let init = base_join("__init__.py");
            if view.paths.contains(&init) {
                files.push(init);
                found = true;
            }
        }
        for n in &imp.names {
            if push_cands(
                &base_join(&if rel.is_empty() {
                    n.clone()
                } else {
                    format!("{rel}/{n}")
                }),
                &mut files,
            ) {
                found = true;
            }
        }
        if !found && rel.is_empty() && imp.names.is_empty() {
            found = true; // `from . import *`-ish with nothing to check
        }
        let mut r = Resolution::new(if found { "local" } else { "unresolved" });
        files.sort();
        files.dedup();
        r.resolved_files = files;
        return r;
    }
    let first = m.split('.').next().unwrap_or(m);
    let mut files = vec![];
    let mut local = false;
    if let Some(f) = view.py_modules.get(m) {
        files.push(f.clone());
        local = true;
    } else if view.py_dirs.contains(m) {
        local = true;
    }
    for n in &imp.names {
        if let Some(f) = view.py_modules.get(&format!("{m}.{n}")) {
            files.push(f.clone());
            local = true;
        }
    }
    if local {
        files.sort();
        files.dedup();
        let mut r = Resolution::new("local");
        r.resolved_files = files;
        return r;
    }
    if PY_STDLIB.contains(&first) {
        return Resolution::new("stdlib");
    }
    if view.local_pkgs.contains(&normalize_pkg(first)) {
        return Resolution::new("local");
    }
    let mut r = Resolution::new("external");
    r.package = Some(first.to_string());
    r
}

const JS_EXTS: &[&str] = &[
    "", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".json", ".d.ts",
];

fn js_try(view: &ProjectView, stem: &str) -> Option<String> {
    // `import './x.js'` in TypeScript may refer to x.ts
    let mut stems = vec![stem.to_string()];
    for e in [".js", ".jsx", ".mjs", ".cjs"] {
        if let Some(s) = stem.strip_suffix(e) {
            stems.push(s.to_string());
        }
    }
    for s in &stems {
        for e in JS_EXTS {
            let c = format!("{s}{e}");
            if view.paths.contains(&c) {
                return Some(c);
            }
        }
        for e in &JS_EXTS[1..] {
            let c = format!("{s}/index{e}");
            if view.paths.contains(&c) {
                return Some(c);
            }
        }
    }
    None
}

fn resolve_js(view: &ProjectView, file_path: &str, imp: &ImportFact) -> Resolution {
    let m = imp.module.as_str();
    if m.starts_with("http://") || m.starts_with("https://") || m.starts_with("data:") {
        return Resolution::new("url");
    }
    if m.starts_with("./") || m.starts_with("../") || m == "." || m == ".." {
        let joined = join_norm(dir_of(file_path), m);
        if let Some(j) = joined {
            if let Some(f) = js_try(view, &j) {
                let mut r = Resolution::new("local");
                r.resolved_files = vec![f];
                return r;
            }
            // `require('..')` / `import '../'`: a directory (or the project root) resolved through
            // its package.json "main" or index file; the entry file may not be one we can name.
            let prefix = if j.is_empty() {
                String::new()
            } else {
                format!("{j}/")
            };
            if view.paths.iter().any(|p| p.starts_with(&prefix)) {
                let mut r = Resolution::new("local");
                let root_index = JS_EXTS[1..]
                    .iter()
                    .map(|e| format!("{prefix}index{e}"))
                    .find(|c| view.paths.contains(c));
                r.resolved_files = root_index.into_iter().collect();
                return r;
            }
        }
        return Resolution::new("unresolved");
    }
    if m.starts_with('/') {
        return Resolution::new("alias");
    }
    if let Some(b) = m.strip_prefix("node:") {
        let _ = b;
        return Resolution::new("stdlib");
    }
    if m.starts_with("@/") || m.starts_with('~') || m.starts_with('#') || m.starts_with('$') {
        return Resolution::new("alias");
    }
    let pkg = if m.starts_with('@') {
        m.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
    } else {
        m.split('/').next().unwrap_or(m).to_string()
    };
    if NODE_BUILTINS.contains(&pkg.as_str()) {
        return Resolution::new("stdlib");
    }
    if view.local_pkgs.contains(&pkg) {
        return Resolution::new("local");
    }
    if m.contains('/') && !m.starts_with('@') {
        if let Some(f) = js_try(view, m) {
            let mut r = Resolution::new("local"); // baseUrl-style import
            r.resolved_files = vec![f];
            return r;
        }
    }
    let mut r = Resolution::new("external");
    r.package = Some(pkg);
    r
}

fn resolve_rust(
    view: &ProjectView,
    file_path: &str,
    imp: &ImportFact,
    bound: &HashSet<String>,
) -> Resolution {
    let m = imp.module.trim_start_matches("::");
    if imp.kind == "mod" {
        let dir = dir_of(file_path);
        let stem = file_path
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".rs");
        // Cargo target roots: tests/x.rs, benches/x.rs, examples/x.rs, src/bin/x.rs are crate roots,
        // so their child modules live next to them (`tests/common/mod.rs`), like main.rs.
        let dir_name = dir.rsplit('/').next().unwrap_or("");
        let is_target_root = matches!(dir_name, "tests" | "benches" | "examples")
            || (dir_name == "bin" && dir.ends_with("src/bin"));
        let base = if matches!(stem, "mod" | "lib" | "main")
            || file_path.ends_with("build.rs")
            || is_target_root
        {
            dir.to_string()
        } else if dir.is_empty() {
            stem.to_string()
        } else {
            format!("{dir}/{stem}")
        };
        let j = |s: &str| {
            if base.is_empty() {
                s.to_string()
            } else {
                format!("{base}/{s}")
            }
        };
        for c in [j(&format!("{}.rs", m)), j(&format!("{}/mod.rs", m))] {
            if view.paths.contains(&c) {
                let mut r = Resolution::new("local");
                r.resolved_files = vec![c];
                return r;
            }
        }
        // `#[path]` modules and generated modules are not resolved
        return Resolution::new("unresolved");
    }
    let first_end = m.find([':', '{', ';', ',']).unwrap_or(m.len());
    let first = m[..first_end].trim();
    if first.is_empty() || first.starts_with(|c: char| c.is_ascii_uppercase()) {
        return Resolution::new("local");
    }
    if matches!(first, "crate" | "self" | "super") {
        let segs: Vec<&str> = m
            .split("::")
            .filter(|s| !matches!(*s, "crate" | "self" | "super" | "*") && !s.starts_with('{'))
            .collect();
        let mut files = vec![];
        // longest prefix of path segments that matches a file suffix
        for n in (1..=segs.len()).rev() {
            let joined = segs[..n].join("/");
            let cands: Vec<&String> = view
                .rust_files
                .iter()
                .filter(|f| {
                    f.ends_with(&format!("/{joined}.rs"))
                        || *f == &format!("{joined}.rs")
                        || f.ends_with(&format!("/{joined}/mod.rs"))
                        || *f == &format!("{joined}/mod.rs")
                })
                .collect();
            if let Some(c) = cands.first() {
                files.push((*c).clone());
                break;
            }
        }
        let mut r = Resolution::new("local");
        r.resolved_files = files;
        return r;
    }
    if RUST_STD.contains(&first) {
        return Resolution::new("stdlib");
    }
    if view.rust_mods.contains(first)
        || view.local_pkgs.contains(&normalize_pkg(first))
        || bound.contains(first)
    {
        return Resolution::new("local"); // module, workspace crate, or a name bound by another `use` in this file
    }
    let mut r = Resolution::new("external");
    r.package = Some(first.to_string());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(files: &[(&str, &str)], mods: &[&str], pkgs: &[&str]) -> ProjectView {
        let f: Vec<(String, String)> = files
            .iter()
            .map(|(p, l)| (p.to_string(), l.to_string()))
            .collect();
        ProjectView::build(
            &f,
            mods.iter().map(|s| s.to_string()).collect(),
            pkgs.iter().map(|s| s.to_string()).collect(),
        )
    }
    fn imp(m: &str, names: &[&str], kind: &str) -> ImportFact {
        ImportFact {
            module: m.into(),
            names: names.iter().map(|s| s.to_string()).collect(),
            line: 1,
            kind: kind.into(),
        }
    }

    #[test]
    fn python_resolution() {
        let v = view(
            &[
                ("src/app/__init__.py", "python"),
                ("src/app/util.py", "python"),
                ("src/app/sub/mod.py", "python"),
            ],
            &[],
            &[],
        );
        let f = "src/app/main.py";
        assert_eq!(
            resolve_import(
                &v,
                Family::Python,
                f,
                &imp("app.util", &[], "import"),
                &HashSet::new()
            )
            .class,
            "local"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Python,
                f,
                &imp(".util", &["x"], "import"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/app/util.py"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Python,
                f,
                &imp(".nope", &[], "import"),
                &HashSet::new()
            )
            .class,
            "unresolved"
        );
        assert!(resolve_import(
            &v,
            Family::Python,
            f,
            &imp(".", &["util"], "import"),
            &HashSet::new()
        )
        .resolved_files
        .contains(&"src/app/util.py".to_string()));
        assert_eq!(
            resolve_import(
                &v,
                Family::Python,
                f,
                &imp("os.path", &[], "import"),
                &HashSet::new()
            )
            .class,
            "stdlib"
        );
        let r = resolve_import(
            &v,
            Family::Python,
            f,
            &imp("requests.adapters", &[], "import"),
            &HashSet::new(),
        );
        assert_eq!(
            (r.class.as_str(), r.package.as_deref()),
            ("external", Some("requests"))
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Python,
                f,
                &imp("app.sub", &[], "import"),
                &HashSet::new()
            )
            .class,
            "local"
        ); // namespace dir
    }

    #[test]
    fn js_resolution() {
        let v = view(
            &[
                ("src/a.ts", "typescript"),
                ("src/lib/index.js", "javascript"),
                ("src/b.js", "javascript"),
            ],
            &[],
            &["@me/core"],
        );
        let f = "src/main.ts";
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("./a", &[], "import"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/a.ts"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("./a.js", &[], "import"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/a.ts"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("./lib", &[], "import"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/lib/index.js"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("./missing", &[], "import"),
                &HashSet::new()
            )
            .class,
            "unresolved"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("node:fs", &[], "import"),
                &HashSet::new()
            )
            .class,
            "stdlib"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("fs/promises", &[], "import"),
                &HashSet::new()
            )
            .class,
            "stdlib"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("@me/core/x", &[], "import"),
                &HashSet::new()
            )
            .class,
            "local"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::JsLike,
                f,
                &imp("@/x", &[], "import"),
                &HashSet::new()
            )
            .class,
            "alias"
        );
        let r = resolve_import(
            &v,
            Family::JsLike,
            f,
            &imp("@scope/pkg/deep/x", &[], "import"),
            &HashSet::new(),
        );
        assert_eq!(r.package.as_deref(), Some("@scope/pkg"));
        let r = resolve_import(
            &v,
            Family::JsLike,
            f,
            &imp("lodash/fp", &[], "require"),
            &HashSet::new(),
        );
        assert_eq!(r.package.as_deref(), Some("lodash"));
    }

    #[test]
    fn rust_use_bindings_cases() {
        let mut b = rust_use_bindings("nix::sys::ptrace");
        b.sort();
        assert_eq!(b, vec!["ptrace"]);
        let mut b = rust_use_bindings("a::b::{c, d::e, f as g, self, h::*}");
        b.sort();
        assert_eq!(b, vec!["b", "c", "e", "g"]);
        assert_eq!(rust_use_bindings("x::y as z"), vec!["z"]);
    }

    #[test]
    fn rust_name_bound_by_other_use_is_local() {
        let v = view(&[("src/lib.rs", "rust")], &[], &[]);
        let bound: HashSet<String> = ["ptrace".to_string()].into();
        let r = resolve_import(
            &v,
            Family::Rust,
            "src/lib.rs",
            &imp("ptrace::Options", &[], "import"),
            &bound,
        );
        assert_eq!(r.class, "local");
        let r = resolve_import(
            &v,
            Family::Rust,
            "src/lib.rs",
            &imp("ptrace::Options", &[], "import"),
            &HashSet::new(),
        );
        assert_eq!(r.class, "external");
    }

    #[test]
    fn rust_resolution() {
        let v = view(
            &[
                ("src/lib.rs", "rust"),
                ("src/a.rs", "rust"),
                ("src/b/mod.rs", "rust"),
            ],
            &["a", "b"],
            &["my-sibling"],
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("a", &[], "mod"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/a.rs"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("b", &[], "mod"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/b/mod.rs"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("zzz", &[], "mod"),
                &HashSet::new()
            )
            .class,
            "unresolved"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("std::fmt", &[], "import"),
                &HashSet::new()
            )
            .class,
            "stdlib"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("crate::a::Thing", &[], "import"),
                &HashSet::new()
            )
            .resolved_files,
            vec!["src/a.rs"]
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("a::f", &[], "import"),
                &HashSet::new()
            )
            .class,
            "local"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("my_sibling::x", &[], "import"),
                &HashSet::new()
            )
            .class,
            "local"
        );
        assert_eq!(
            resolve_import(
                &v,
                Family::Rust,
                "src/lib.rs",
                &imp("Direction::*", &[], "import"),
                &HashSet::new()
            )
            .class,
            "local"
        );
        let r = resolve_import(
            &v,
            Family::Rust,
            "src/lib.rs",
            &imp("serde::{Serialize,Deserialize}", &[], "import"),
            &HashSet::new(),
        );
        assert_eq!(
            (r.class.as_str(), r.package.as_deref()),
            ("external", Some("serde"))
        );
    }

    #[test]
    fn rust_cargo_target_roots_resolve_child_modules_like_main_rs() {
        let v = view(
            &[
                ("crates/c/tests/a.rs", "rust"),
                ("crates/c/tests/common/mod.rs", "rust"),
                ("crates/c/src/bin/tool.rs", "rust"),
                ("crates/c/src/bin/helper.rs", "rust"),
                ("crates/c/src/lib.rs", "rust"),
                ("crates/c/src/util.rs", "rust"),
            ],
            &[],
            &[],
        );
        let r = |from: &str, m: &str| {
            resolve_import(&v, Family::Rust, from, &imp(m, &[], "mod"), &HashSet::new())
        };
        assert_eq!(r("crates/c/tests/a.rs", "common").class, "local");
        assert_eq!(r("crates/c/src/bin/tool.rs", "helper").class, "local");
        // a normal module file still looks in its own directory
        assert_eq!(r("crates/c/src/util.rs", "nope").class, "unresolved");
    }
}
