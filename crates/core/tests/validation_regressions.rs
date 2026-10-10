//! Regressions found by running Plumbgraph on real repositories (docs/VALIDATION.md).
//! Each test names the repo where the bug was observed.
use plumbgraph_core::ops::{run_check_deps, run_dead_code, DeadCodeParams, DepsParams, Target};
use plumbgraph_core::registry::{Registry, StaticRegistry};
use std::collections::HashMap;
use std::path::Path;

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn target(t: &tempfile::TempDir) -> Target {
    Target {
        root: t.path().to_path_buf(),
        db: Some(t.path().join(".plumbgraph/i.db")),
    }
}

fn deps(t: &tempfile::TempDir, known: &[(&str, &str, bool)]) -> plumbgraph_core::deps::DepsReport {
    let mut m = HashMap::new();
    for (e, n, x) in known {
        m.insert((e.to_string(), n.to_string()), *x);
    }
    let r = StaticRegistry { known: m };
    run_check_deps(
        &target(t),
        &DepsParams::default(),
        Some(&r as &dyn Registry),
    )
    .unwrap()
}

fn dead_names(t: &tempfile::TempDir) -> Vec<String> {
    run_dead_code(&target(t), &DeadCodeParams::default())
        .unwrap()
        .findings
        .iter()
        .map(|f| f.symbol.clone().unwrap_or_default())
        .collect()
}

/// ripgrep: `use doc::{generate_long as generate_help_long}` made `generate_long` a HIGH
/// confidence dead-code false positive.
#[test]
fn rust_aliased_use_keeps_the_original_alive() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "Cargo.toml",
        "[package]\nname = \"al\"\nversion = \"0.1.0\"\n",
    );
    write(
        t.path(),
        "src/main.rs",
        "mod util;\nuse util::{helper as h, other::deep as d};\nfn main() { h(); d(); }\n",
    );
    write(
        t.path(),
        "src/util.rs",
        "pub mod other { pub fn deep() {} }\npub fn helper() {}\npub fn truly_unused() {}\n",
    );
    let dead = dead_names(&t);
    assert!(!dead.iter().any(|s| s.ends_with("helper")), "{dead:?}");
    assert!(!dead.iter().any(|s| s.ends_with("deep")), "{dead:?}");
    assert!(dead.iter().any(|s| s.ends_with("truly_unused")), "{dead:?}");
}

#[test]
fn python_and_js_aliased_imports_keep_the_original_alive() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "a.py",
        "from b import helper as h\n\ndef main():\n    h()\n\nmain()\n",
    );
    write(
        t.path(),
        "b.py",
        "def helper(): pass\ndef py_unused(): pass\n",
    );
    write(
        t.path(),
        "a.js",
        "import { helper as h } from './b.js';\nh();\n",
    );
    write(
        t.path(),
        "b.js",
        "export function helper() {}\nexport function js_unused() {}\n",
    );
    let dead = run_dead_code(
        &target(&t),
        &DeadCodeParams {
            min_confidence: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    let names: Vec<_> = dead
        .findings
        .iter()
        .map(|f| (f.file.as_str(), f.symbol.clone().unwrap_or_default()))
        .collect();
    assert!(!names.iter().any(|(_, s)| s == "helper"), "{names:?}");
    assert!(names.iter().any(|(_, s)| s == "py_unused"));
    assert!(names.iter().any(|(_, s)| s == "js_unused"));
}

/// express: `require('..')` from test/ (project root with index.js) was `unresolved-import`.
#[test]
fn js_directory_and_root_imports_resolve() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "package.json",
        "{\"name\":\"x\",\"main\":\"index.js\"}\n",
    );
    write(
        t.path(),
        "index.js",
        "module.exports = require('./lib/app');\n",
    );
    write(t.path(), "lib/app.js", "exports.go = function () {};\n");
    write(
        t.path(),
        "test/a.js",
        "var x = require('..');\nvar y = require('../');\nvar z = require('../lib');\nx.go(); y.go(); z.go();\n",
    );
    let rep = deps(&t, &[]);
    let bad: Vec<_> = rep.findings.iter().map(|f| (&f.rule, &f.symbol)).collect();
    assert!(
        !rep.findings.iter().any(|f| f.rule == "unresolved-import"),
        "{bad:?}"
    );
    // a genuinely missing relative import is still reported
    write(t.path(), "test/b.js", "require('./nope');\n");
    let rep = deps(&t, &[]);
    assert!(rep
        .findings
        .iter()
        .any(|f| f.rule == "unresolved-import" && f.file == "test/b.js"));
}

/// Newtonsoft.Json: every `using Newtonsoft.Json.Linq;` / `using Xunit;` was a MEDIUM
/// unresolved-import (2904 findings). A namespace is not a file path.
#[test]
fn csharp_using_namespaces_are_not_unresolved_imports() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "Src/A.cs",
        "using System;\nusing Xunit;\nusing Acme.Json.Linq;\nnamespace Acme.Json { public class A { public void M() {} } }\n",
    );
    let rep = deps(&t, &[]);
    assert!(
        !rep.findings.iter().any(|f| f.rule == "unresolved-import"),
        "{:?}",
        rep.findings.iter().map(|f| &f.symbol).collect::<Vec<_>>()
    );
}

/// requests: `try: import StringIO / except ImportError:` (py2 compat) was a MEDIUM
/// nonexistent-package finding.
#[test]
fn python_import_error_guarded_imports_are_low_confidence() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "requirements.txt", "requests\n");
    write(
        t.path(),
        "compat.py",
        "try:\n    import StringIO\nexcept ImportError:\n    import io as StringIO\n\nimport reallyfakepkg\n",
    );
    let rep = deps(
        &t,
        &[
            ("pypi", "StringIO", false),
            ("pypi", "reallyfakepkg", false),
            ("pypi", "requests", true),
        ],
    );
    let guarded = rep
        .findings
        .iter()
        .find(|f| f.symbol.as_deref() == Some("StringIO"))
        .expect("still reported, but weakly");
    assert!(guarded.confidence < 0.5, "{}", guarded.confidence);
    assert!(guarded.fp_risks.iter().any(|r| r.contains("ImportError")));
    let real = rep
        .findings
        .iter()
        .find(|f| f.symbol.as_deref() == Some("reallyfakepkg"))
        .unwrap();
    assert!(real.confidence >= 0.7, "unguarded fake import stays strong");
}
