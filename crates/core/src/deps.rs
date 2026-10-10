//! Import-vs-manifest checks and registry existence checks ("hallucinated package" detection).

use crate::manifest::Manifest;
use crate::registry::{Existence, Registry};
use crate::resolve::normalize_pkg;
use crate::store::Graph;
use crate::{sanitize, Finding, Severity};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Well-known import-name -> PyPI distribution-name differences.
pub const PY_IMPORT_TO_DIST: &[(&str, &str)] = &[
    ("PIL", "pillow"),
    ("yaml", "pyyaml"),
    ("cv2", "opencv-python"),
    ("sklearn", "scikit-learn"),
    ("bs4", "beautifulsoup4"),
    ("dateutil", "python-dateutil"),
    ("dotenv", "python-dotenv"),
    ("OpenSSL", "pyopenssl"),
    ("serial", "pyserial"),
    ("jwt", "pyjwt"),
    ("attr", "attrs"),
    ("git", "gitpython"),
    ("magic", "python-magic"),
    ("skimage", "scikit-image"),
    ("docx", "python-docx"),
    ("pkg_resources", "setuptools"),
    ("Crypto", "pycryptodome"),
    ("usb", "pyusb"),
    ("google", "protobuf"),
    ("markdown", "markdown"),
    ("psycopg2", "psycopg2-binary"),
    ("MySQLdb", "mysqlclient"),
    ("zmq", "pyzmq"),
    ("lxml", "lxml"),
    ("win32api", "pywin32"),
    ("slugify", "python-slugify"),
    ("jose", "python-jose"),
    ("socks", "pysocks"),
    ("ruamel", "ruamel.yaml"),
    ("fitz", "pymupdf"),
    ("_pytest", "pytest"),
    ("setuptools", "setuptools"),
    ("pytest", "pytest"),
    ("numpy", "numpy"),
    ("pandas", "pandas"),
];

#[derive(Debug, Clone)]
pub struct DepsOptions {
    pub path_prefix: Option<String>,
    pub max_lookups: usize,
    /// Also look up every checkable dependency declared in manifests (not only undeclared imports).
    pub check_declared: bool,
}

impl Default for DepsOptions {
    fn default() -> Self {
        DepsOptions {
            path_prefix: None,
            max_lookups: 300,
            check_declared: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Lookup {
    pub ecosystem: String,
    pub package: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DepsReport {
    pub findings: Vec<Finding>,
    pub lookups: Vec<Lookup>,
    pub registry: String,
    pub offline: bool,
    pub imports_checked: usize,
    pub external_imports: usize,
}

fn eco_for(lang: &str) -> Option<&'static str> {
    match lang {
        "python" => Some("pypi"),
        "javascript" | "typescript" | "tsx" => Some("npm"),
        "rust" => Some("crates.io"),
        _ => None,
    }
}

fn norm_for(eco: &str, name: &str) -> String {
    match eco {
        "crates.io" => name.to_ascii_lowercase().replace('-', "_"),
        "pypi" => normalize_pkg(name),
        _ => name.to_string(),
    }
}

fn py_candidates(import_name: &str) -> (Vec<String>, Option<&'static str>) {
    let mapped = PY_IMPORT_TO_DIST
        .iter()
        .find(|(i, _)| *i == import_name)
        .map(|(_, d)| *d);
    let mut c = vec![normalize_pkg(import_name)];
    if let Some(m) = mapped {
        c.push(normalize_pkg(m));
    }
    (c, mapped)
}

fn py_relaxed(declared: &str, import_norm: &str) -> bool {
    let d = declared
        .trim_start_matches("python-")
        .trim_start_matches("py-")
        .trim_end_matches("-python")
        .trim_end_matches("-py");
    d == import_norm
}

fn ancestor_manifests<'a>(manifests: &'a [Manifest], eco: &str, file: &str) -> Vec<&'a Manifest> {
    manifests
        .iter()
        .filter(|m| m.eco == eco && (m.dir.is_empty() || file.starts_with(&format!("{}/", m.dir))))
        .collect()
}

pub fn check_deps(
    graph: &Graph,
    manifests: &[Manifest],
    registry: &dyn Registry,
    opts: &DepsOptions,
) -> DepsReport {
    let offline = registry.name() == "offline";
    let files: HashMap<i64, &crate::store::FileRow> =
        graph.files.iter().map(|f| (f.id, f)).collect();
    let in_scope = |path: &str| {
        opts.path_prefix
            .as_ref()
            .map(|p| path.starts_with(p.as_str()))
            .unwrap_or(true)
    };

    struct Ext {
        file: String,
        line: u32,
        module: String,
        eco: &'static str,
        import_name: String,
        declared: bool,
        reg_name: String,
        mapped: bool,
        has_manifest: bool,
        dev_only: bool,
        /// inside `try: import x / except ImportError`: an optional or compat import
        optional: bool,
    }
    let mut report = DepsReport {
        findings: vec![],
        lookups: vec![],
        registry: registry.name().into(),
        offline,
        imports_checked: 0,
        external_imports: 0,
    };
    let mut exts: Vec<Ext> = vec![];

    for imp in &graph.imports {
        let Some(f) = files.get(&imp.file_id) else {
            continue;
        };
        if !in_scope(&f.path) || f.is_generated {
            continue;
        }
        report.imports_checked += 1;
        if imp.class == "unresolved" {
            let (conf, risk) = if imp.kind == "mod" {
                (
                    0.60,
                    "a `#[path]` attribute or build-generated module may exist",
                )
            } else {
                (0.80, "the target may be generated at build time or excluded from the index (gitignore/vendor dirs)")
            };
            let mut fd = Finding::new(
                "deps",
                "unresolved-import",
                &f.path,
                imp.line,
                conf,
                "t0-import-resolution",
                format!(
                    "import `{}` does not resolve to any indexed file",
                    sanitize(&imp.module)
                ),
            );
            fd.severity = Severity::Error;
            fd.kind = "import".into();
            fd.symbol = Some(sanitize(&imp.module));
            fd.evidence = vec![format!(
                "no file matching `{}` found relative to {}",
                sanitize(&imp.module),
                f.path
            )];
            fd.fp_risks = vec![risk.into()];
            report.findings.push(fd);
            continue;
        }
        if imp.class != "external" {
            continue;
        }
        let Some(pkg) = imp.package.as_ref() else {
            continue;
        };
        let Some(eco) = eco_for(&f.lang) else {
            continue;
        };
        report.external_imports += 1;
        let optional = imp.kind == "optional";
        let anc = ancestor_manifests(manifests, eco, &f.path);
        let want = norm_for(eco, pkg);
        let (cands, mapped_dist) = if eco == "pypi" {
            py_candidates(pkg)
        } else {
            (vec![want.clone()], None)
        };
        let mut declared = false;
        let mut dev_only = true;
        for m in &anc {
            for d in &m.deps {
                let dn = norm_for(eco, &d.name);
                let hit = if eco == "pypi" {
                    cands.contains(&dn) || py_relaxed(&dn, &want)
                } else {
                    dn == want
                };
                if hit {
                    declared = true;
                    let dev = d.section.contains("dev")
                        || d.section.contains("test")
                        || d.section.contains("build-system");
                    if !dev {
                        dev_only = false;
                    }
                }
            }
        }
        exts.push(Ext {
            file: f.path.clone(),
            line: imp.line,
            module: imp.module.clone(),
            eco,
            import_name: pkg.clone(),
            declared,
            reg_name: if eco == "pypi" {
                mapped_dist.map(String::from).unwrap_or_else(|| pkg.clone())
            } else {
                pkg.clone()
            },
            mapped: mapped_dist.is_some(),
            has_manifest: !anc.is_empty(),
            dev_only: declared && dev_only && !f.is_test,
            optional,
        });
    }

    // lookups
    let mut to_look: BTreeSet<(String, String)> = BTreeSet::new();
    for e in &exts {
        if !e.declared {
            to_look.insert((e.eco.to_string(), e.reg_name.clone()));
        }
    }
    let local_names: HashSet<String> = manifests
        .iter()
        .filter_map(|m| m.package_name.as_ref().map(|n| norm_for(m.eco, n)))
        .collect();
    if opts.check_declared {
        for m in manifests {
            let relevant = opts
                .path_prefix
                .as_ref()
                .map(|p| m.dir.starts_with(p.as_str()) || p.starts_with(&m.dir))
                .unwrap_or(true);
            if !relevant {
                continue;
            }
            for d in &m.deps {
                if d.checkable && !local_names.contains(&norm_for(m.eco, &d.registry_name)) {
                    to_look.insert((m.eco.to_string(), d.registry_name.clone()));
                }
            }
        }
    }
    let list: Vec<(String, String)> = to_look.into_iter().collect();
    let capped = list.len() > opts.max_lookups;
    let results: Vec<((String, String), Existence)> = list
        .par_iter()
        .enumerate()
        .map(|(i, k)| {
            if i >= opts.max_lookups {
                (k.clone(), Existence::Unknown("lookup cap reached".into()))
            } else {
                (k.clone(), registry.lookup(&k.0, &k.1))
            }
        })
        .collect();
    let _ = capped;
    let res: BTreeMap<(String, String), Existence> = results.into_iter().collect();
    for ((eco, name), r) in &res {
        report.lookups.push(Lookup {
            ecosystem: eco.clone(),
            package: name.clone(),
            result: match r {
                Existence::Exists => "exists".into(),
                Existence::NotFound => "not-found".into(),
                Existence::Unknown(why) => format!("unknown ({why})"),
            },
        });
    }

    // import findings
    for e in &exts {
        if e.declared {
            continue;
        }
        let r = res.get(&(e.eco.to_string(), e.reg_name.clone()));
        let py_unmapped = e.eco == "pypi" && !e.mapped;
        let reg_src = format!("registry:{}", e.eco);
        let mut ev = vec![format!(
            "`{}` is imported at {}:{}",
            sanitize(&e.import_name),
            e.file,
            e.line
        )];
        let mut fp: Vec<String> = vec![];
        if e.has_manifest {
            ev.push(format!(
                "not declared in any {} manifest covering this file",
                e.eco
            ));
        } else {
            ev.push(format!("no {} manifest found for this file", e.eco));
            fp.push("no manifest was found, so declared dependencies are unknown".into());
        }
        if e.eco == "pypi" {
            fp.push("a Python import name can differ from its distribution name (known mappings are built in; others are not)".into());
        }
        let (rule, mut conf, src, msg): (&str, f64, String, String) = match r {
            Some(Existence::NotFound) => {
                ev.push(format!(
                    "{} returned 404 for `{}`",
                    e.eco,
                    sanitize(&e.reg_name)
                ));
                (
                    "nonexistent-package",
                    if py_unmapped {
                        0.70
                    } else if e.eco == "pypi" {
                        0.85
                    } else {
                        0.90
                    },
                    reg_src,
                    format!(
                        "`{}` is not declared and does not exist on {}",
                        sanitize(&e.import_name),
                        e.eco
                    ),
                )
            }
            other => {
                match other {
                    Some(Existence::Exists) => ev.push(format!(
                        "{} has a package named `{}`",
                        e.eco,
                        sanitize(&e.reg_name)
                    )),
                    Some(Existence::Unknown(why)) => {
                        ev.push(format!("registry not consulted or inconclusive: {why}"))
                    }
                    _ => ev.push("registry not consulted".into()),
                }
                (
                    "undeclared-dependency",
                    if py_unmapped {
                        0.55
                    } else if e.eco == "pypi" {
                        0.70
                    } else {
                        0.80
                    },
                    "manifest+t0-import".to_string(),
                    format!(
                        "`{}` is imported but not declared in a manifest",
                        sanitize(&e.import_name)
                    ),
                )
            }
        };
        if !e.has_manifest {
            conf = conf.min(0.45);
        }
        if e.optional {
            // guarded by `except ImportError`: absence is expected (py2 compat, optional extras)
            conf = conf.min(0.35);
            fp.push("the import is guarded by try/except ImportError (optional or compatibility import)".into());
        }
        let mut fd = Finding::new("deps", rule, &e.file, e.line, conf, &src, msg);
        fd.severity = Severity::Error;
        fd.kind = "import".into();
        fd.symbol = Some(sanitize(&e.module));
        fd.evidence = ev;
        fd.fp_risks = fp;
        report.findings.push(fd);
    }
    // declared-but-nonexistent
    for m in manifests {
        for d in &m.deps {
            if !d.checkable {
                continue;
            }
            if let Some(Existence::NotFound) =
                res.get(&(m.eco.to_string(), d.registry_name.clone()))
            {
                let mut fd = Finding::new(
                    "deps",
                    "nonexistent-declared-package",
                    &m.path,
                    d.line,
                    0.90,
                    &format!("registry:{}", m.eco),
                    format!(
                        "`{}` is declared in {} but does not exist on {}",
                        sanitize(&d.registry_name),
                        m.path,
                        m.eco
                    ),
                );
                fd.severity = Severity::Error;
                fd.kind = "dependency".into();
                fd.symbol = Some(sanitize(&d.name));
                fd.evidence = vec![
                    format!("declared in section `{}`", d.section),
                    format!("{} returned 404", m.eco),
                ];
                fd.fp_risks = vec!["a private/alternative registry configured outside the manifest would not be visible to this check".into()];
                report.findings.push(fd);
            }
        }
    }
    // dev-only dependency used in non-test source: informational
    for e in &exts {
        if e.dev_only {
            let mut fd = Finding::new(
                "deps",
                "dev-dependency-in-source",
                &e.file,
                e.line,
                0.5,
                "manifest+t0-import",
                format!(
                    "`{}` is only declared as a dev/build dependency",
                    sanitize(&e.import_name)
                ),
            );
            fd.severity = Severity::Info;
            fd.kind = "import".into();
            fd.evidence = vec!["declared only in a dev/test/build section".into()];
            fd.fp_risks = vec!["the importing file may itself be test/build-only code".into()];
            report.findings.push(fd);
        }
    }
    report.findings.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    report
}
