mod common;
use common::*;
use plumbgraph_core::ops::{run_dead_code, DeadCodeParams, Target};

fn run(name: &str, p: DeadCodeParams) -> Vec<plumbgraph_core::Finding> {
    let t = copy_fixture(name);
    let dbdir = tempfile::tempdir().unwrap();
    let r = run_dead_code(
        &Target {
            root: t.path().to_path_buf(),
            db: Some(dbdir.path().join("i.db")),
        },
        &p,
    )
    .unwrap();
    r.findings
}

#[test]
fn python_default() {
    assert_golden(
        "python_default",
        &golden_lines(&run("python", DeadCodeParams::default())),
    );
}

#[test]
fn python_include_low() {
    assert_golden(
        "python_low",
        &golden_lines(&run(
            "python",
            DeadCodeParams {
                min_confidence: 0.0,
                ..Default::default()
            },
        )),
    );
}

#[test]
fn python_lib_mode_treats_exports_as_api() {
    let f = run(
        "python",
        DeadCodeParams {
            library_mode: true,
            ..Default::default()
        },
    );
    let names: Vec<String> = f.iter().filter_map(|x| x.symbol.clone()).collect();
    assert!(!names.contains(&"unused_public".to_string()), "{names:?}");
    assert!(names.contains(&"_dead_private".to_string()));
}

#[test]
fn javascript_default() {
    assert_golden(
        "js_default",
        &golden_lines(&run("js", DeadCodeParams::default())),
    );
}

#[test]
fn typescript_default() {
    assert_golden(
        "ts_default",
        &golden_lines(&run("ts", DeadCodeParams::default())),
    );
}

#[test]
fn rust_default() {
    assert_golden(
        "rust_default",
        &golden_lines(&run("rust", DeadCodeParams::default())),
    );
}

#[test]
fn names_used_from_html_are_live_but_flagged_without_it() {
    let with_html = run("js", DeadCodeParams::default());
    assert!(!with_html
        .iter()
        .any(|f| f.symbol.as_deref() == Some("htmlOnly")));
    let t = copy_fixture("js");
    std::fs::remove_file(t.path().join("index.html")).unwrap();
    let dbdir = tempfile::tempdir().unwrap();
    let r = run_dead_code(
        &Target {
            root: t.path().to_path_buf(),
            db: Some(dbdir.path().join("i.db")),
        },
        &DeadCodeParams::default(),
    )
    .unwrap();
    assert!(
        r.findings
            .iter()
            .any(|f| f.symbol.as_deref() == Some("htmlOnly")),
        "without the html file the export is unused"
    );
}
