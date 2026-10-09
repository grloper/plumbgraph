mod common;
use common::*;
use plumbgraph_core::ops::{run_check_deps, DepsParams, Target};
use plumbgraph_core::registry::StaticRegistry;
use std::collections::HashMap;

fn reg(items: &[(&str, &str, bool)]) -> StaticRegistry {
    let mut known = HashMap::new();
    for (e, n, x) in items {
        known.insert((e.to_string(), n.to_string()), *x);
    }
    StaticRegistry { known }
}

fn run(name: &str, r: Option<&StaticRegistry>, offline: bool) -> plumbgraph_core::deps::DepsReport {
    let t = copy_fixture(name);
    let db = tempfile::tempdir().unwrap();
    run_check_deps(
        &Target {
            root: t.path().to_path_buf(),
            db: Some(db.path().join("i.db")),
        },
        &DepsParams {
            offline,
            ..Default::default()
        },
        r.map(|r| r as &dyn plumbgraph_core::registry::Registry),
    )
    .unwrap()
}

#[test]
fn python_deps() {
    let r = reg(&[
        ("pypi", "requests", true),
        ("pypi", "PyYAML", true),
        ("pypi", "pyyaml", true),
        ("pypi", "numpy", true),
        ("pypi", "totallyfakepkg", false),
        ("pypi", "hallucinated-lib-xyz", false),
    ]);
    let rep = run("deps_py", Some(&r), false);
    assert_golden("deps_py", &golden_lines(&rep.findings));
    // declared & mapped imports produce no findings
    assert!(!rep
        .findings
        .iter()
        .any(|f| f.symbol.as_deref() == Some("yaml") || f.symbol.as_deref() == Some("requests")));
}

#[test]
fn js_deps() {
    let r = reg(&[
        ("npm", "express", true),
        ("npm", "ghost-pkg-zzz", false),
        ("npm", "jest", true),
        ("npm", "lodash", true),
        ("npm", "fake-undeclared-pkg", false),
    ]);
    let rep = run("deps_js", Some(&r), false);
    assert_golden("deps_js", &golden_lines(&rep.findings));
}

#[test]
fn rust_deps() {
    let r = reg(&[
        ("crates.io", "serde", true),
        ("crates.io", "ghostcrate", false),
        ("crates.io", "rand", true),
    ]);
    let rep = run("deps_rust", Some(&r), false);
    assert_golden("deps_rust", &golden_lines(&rep.findings));
}

#[test]
fn offline_never_reports_nonexistent_and_says_so() {
    let rep = run("deps_py", None, true);
    assert!(rep.offline);
    assert!(
        !rep.findings.iter().any(|f| f.rule.contains("nonexistent")),
        "offline must not claim nonexistence"
    );
    let fake = rep
        .findings
        .iter()
        .find(|f| f.symbol.as_deref() == Some("totallyfakepkg"))
        .unwrap();
    assert_eq!(fake.rule, "undeclared-dependency");
    assert!(
        fake.evidence.iter().any(|e| e.contains("offline")),
        "{:?}",
        fake.evidence
    );
    assert!(rep.lookups.iter().all(|l| l.result.starts_with("unknown")));
}

#[test]
fn inconclusive_lookup_is_not_nonexistent() {
    // registry knows nothing -> Unknown for everything
    let rep = run("deps_js", Some(&reg(&[])), false);
    assert!(!rep.findings.iter().any(|f| f.rule.contains("nonexistent")));
    assert!(rep
        .findings
        .iter()
        .any(|f| f.rule == "undeclared-dependency"));
}
