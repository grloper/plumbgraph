//! Repo map (ranked, budgeted, changed-file aware) and impact analysis.
use plumbgraph_core::map::{run_impact, run_map, ImpactParams, ImpactTarget, MapParams};
use plumbgraph_core::ops::Target;
use std::path::Path;
use std::process::Command;

fn write(root: &Path, rel: &str, c: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, c).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let s = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    assert!(s.success());
}

fn project() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(
        r,
        "core.py",
        "def core():\n    return 1\n\ndef lonely():\n    return 2\n",
    );
    write(
        r,
        "a.py",
        "from core import core\n\ndef a():\n    return core()\n",
    );
    write(
        r,
        "b.py",
        "from core import core\n\ndef b():\n    return core()\n",
    );
    write(
        r,
        "c.py",
        "from a import a\n\ndef c():\n    return a()\n\ndef other():\n    return 3\n",
    );
    write(
        r,
        "tests/test_a.py",
        "from a import a\n\ndef test_a():\n    assert a() == 1\n",
    );
    git(r, &["init", "-q"]);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "init"]);
    t
}

fn target(t: &tempfile::TempDir) -> Target {
    Target {
        root: t.path().to_path_buf(),
        db: None,
    }
}

#[test]
fn map_ranks_most_referenced_first_and_respects_budget() {
    let t = project();
    let m = run_map(&target(&t), &MapParams::default()).unwrap();
    let names: Vec<&str> = m
        .files
        .iter()
        .flat_map(|f| f.symbols.iter().map(|s| s.name.as_str()))
        .collect();
    assert!(names.contains(&"core"), "{names:?}");
    // `core` has two callers and must outrank the unreferenced `lonely`
    let core = m.symbol_rank("core").unwrap();
    let lonely = m.symbol_rank("lonely").unwrap();
    assert!(core > lonely, "core {core} lonely {lonely}");
    assert!(m.tokens_estimated <= m.budget);
    assert!(m.text.contains("core.py"));

    let small = run_map(
        &target(&t),
        &MapParams {
            tokens: 40,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(small.tokens_estimated <= 40, "{}", small.tokens_estimated);
    assert!(small.shown_symbols < m.shown_symbols);
    assert!(small.shown_symbols >= 1);
    // top-ranked symbol survives the cut
    assert!(small.text.contains("core"));
}

#[test]
fn map_is_changed_file_aware() {
    let t = project();
    write(
        t.path(),
        "c.py",
        "from a import a\n\ndef c():\n    return a() + 1\n\ndef other():\n    return 3\n",
    );
    let base = run_map(&target(&t), &MapParams::default()).unwrap();
    let ch = run_map(
        &target(&t),
        &MapParams {
            changed: Some("HEAD".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(ch.changed_files, vec!["c.py".to_string()]);
    let (b, c) = (
        base.symbol_rank("other").unwrap(),
        ch.symbol_rank("other").unwrap(),
    );
    assert!(c > b, "changed-file symbols must gain rank: {b} -> {c}");
}

#[test]
fn impact_of_symbol_lists_transitive_callers_and_tests() {
    let t = project();
    let r = run_impact(
        &target(&t),
        &ImpactParams {
            target: ImpactTarget::Symbol("core".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let by: std::collections::HashMap<&str, u32> = r
        .affected
        .iter()
        .map(|a| (a.name.as_str(), a.distance))
        .collect();
    assert_eq!(by.get("a"), Some(&1), "{by:?}");
    assert_eq!(by.get("b"), Some(&1));
    assert_eq!(by.get("c"), Some(&2));
    assert_eq!(by.get("test_a"), Some(&2));
    assert!(!by.contains_key("lonely"));
    assert!(r.tests_to_run.contains(&"tests/test_a.py".to_string()));
    assert!(r
        .affected
        .iter()
        .all(|a| a.confidence > 0.0 && !a.source.is_empty()));
}

#[test]
fn impact_of_diff_maps_changed_lines_to_symbols() {
    let t = project();
    write(
        t.path(),
        "core.py",
        "def core():\n    return 42\n\ndef lonely():\n    return 2\n",
    );
    let r = run_impact(
        &target(&t),
        &ImpactParams {
            target: ImpactTarget::Diff("HEAD".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        r.seeds,
        vec!["core".to_string()],
        "only the edited function is a seed"
    );
    assert!(r.affected.iter().any(|a| a.name == "a"));
    assert!(r.tests_to_run.contains(&"tests/test_a.py".to_string()));
}

#[test]
fn impact_unknown_symbol_is_an_error() {
    let t = project();
    let e = run_impact(
        &target(&t),
        &ImpactParams {
            target: ImpactTarget::Symbol("does_not_exist".into()),
            ..Default::default()
        },
    );
    assert!(e.is_err());
}

#[test]
fn option_like_revisions_are_rejected() {
    let t = project();
    let e = run_map(
        &target(&t),
        &MapParams {
            changed: Some("--output=/tmp/x".into()),
            ..Default::default()
        },
    );
    assert!(e.is_err());
}
