//! SCIP ingestion: precise references override name-based edges, with fallback.
mod common;
use plumbgraph_core::ops::{
    references, references_scip, run_dead_code, DeadCodeParams, ScipOpts, Target,
};
use plumbgraph_core::Finding;
use protobuf::{Enum, EnumOrUnknown, Message};
use scip::types::{
    symbol_information::Kind, Document, Index, Metadata, Occurrence, SymbolInformation, SymbolRole,
    ToolInfo,
};
use std::path::Path;

const A_PY: &str = "class A:\n    def run(self):\n        return 1\nclass B:\n    def run(self):\n        return 2\ndef main():\n    A().run()\nmain()\n";

fn occ(sym: &str, line: i32, c0: i32, c1: i32, def: bool) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = sym.into();
    o.range = vec![line, c0, c1];
    if def {
        o.symbol_roles = SymbolRole::Definition.value();
    }
    o
}

fn info(sym: &str, name: &str) -> SymbolInformation {
    let mut i = SymbolInformation::new();
    i.symbol = sym.into();
    i.display_name = name.into();
    i.kind = EnumOrUnknown::new(Kind::Method);
    i
}

const SA: &str = "scip-python python pkg 0.1 `a`/A#";
const SAR: &str = "scip-python python pkg 0.1 `a`/A#run().";
const SB: &str = "scip-python python pkg 0.1 `a`/B#";
const SBR: &str = "scip-python python pkg 0.1 `a`/B#run().";
const SM: &str = "scip-python python pkg 0.1 `a`/main().";

/// Write a SCIP index that resolves `A().run()` to A#run only.
fn write_index(path: &Path) {
    let mut d = Document::new();
    d.relative_path = "a.py".into();
    d.language = "python".into();
    d.occurrences = vec![
        occ(SA, 0, 6, 7, true),
        occ(SAR, 1, 8, 11, true),
        occ(SB, 3, 6, 7, true),
        occ(SBR, 4, 8, 11, true),
        occ(SM, 6, 4, 8, true),
        occ(SA, 7, 4, 5, false),
        occ(SAR, 7, 8, 11, false),
        occ(SM, 8, 0, 4, false),
    ];
    d.symbols = vec![
        info(SA, "A"),
        info(SAR, "run"),
        info(SB, "B"),
        info(SBR, "run"),
        info(SM, "main"),
    ];
    let mut tool = ToolInfo::new();
    tool.name = "scip-python".into();
    tool.version = "0.0-test".into();
    let mut m = Metadata::new();
    m.tool_info = Some(tool).into();
    let mut idx = Index::new();
    idx.metadata = Some(m).into();
    idx.documents = vec![d];
    std::fs::write(path, idx.write_to_bytes().unwrap()).unwrap();
}

fn project(with_scip: bool) -> (tempfile::TempDir, tempfile::TempDir) {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.py"), A_PY).unwrap();
    if with_scip {
        // written after the sources so the index is not considered stale
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_index(&t.path().join("index.scip"));
    }
    (t, tempfile::tempdir().unwrap())
}

fn target(t: &tempfile::TempDir, d: &tempfile::TempDir) -> Target {
    Target {
        root: t.path().to_path_buf(),
        db: Some(d.path().join("i.db")),
    }
}

fn dead(t: &tempfile::TempDir, d: &tempfile::TempDir, scip: ScipOpts) -> Vec<Finding> {
    run_dead_code(
        &target(t, d),
        &DeadCodeParams {
            min_confidence: 0.0,
            scip,
            ..Default::default()
        },
    )
    .unwrap()
    .findings
}

#[test]
fn name_based_cannot_tell_the_two_run_methods_apart() {
    let (t, d) = project(false);
    let f = dead(&t, &d, ScipOpts::default());
    assert!(
        !f.iter().any(|x| x.symbol.as_deref() == Some("B.run")),
        "name-based analysis sees `.run` and keeps both alive: {f:?}"
    );
}

#[test]
fn scip_finds_the_dead_method_and_marks_the_source() {
    let (t, d) = project(true);
    let f = dead(&t, &d, ScipOpts::default());
    let b = f
        .iter()
        .find(|x| x.symbol.as_deref() == Some("B.run"))
        .unwrap_or_else(|| panic!("B.run should be dead with SCIP: {f:?}"));
    assert_eq!(b.source, "scip");
    // public method: the exported penalty still applies (callers outside the index are possible)
    assert!(b.confidence >= 0.7, "{}", b.confidence);
    assert!(
        b.evidence.iter().any(|e| e.contains("SCIP")),
        "{:?}",
        b.evidence
    );
    assert!(!f.iter().any(|x| x.symbol.as_deref() == Some("A.run")));
}

#[test]
fn scip_can_be_disabled_and_falls_back() {
    let (t, d) = project(true);
    let f = dead(
        &t,
        &d,
        ScipOpts {
            disable: true,
            ..Default::default()
        },
    );
    assert!(!f.iter().any(|x| x.symbol.as_deref() == Some("B.run")));
    assert!(f.iter().all(|x| x.source != "scip"));
}

#[test]
fn stale_index_is_ignored_for_changed_files() {
    let (t, d) = project(true);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(t.path().join("a.py"), format!("{A_PY}# edited\n")).unwrap();
    let r = run_dead_code(
        &target(&t, &d),
        &DeadCodeParams {
            min_confidence: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.findings.iter().all(|x| x.source != "scip"));
    let st = r.scip.expect("stats present when an index was found");
    assert_eq!(st.stale_files, 1);
}

#[test]
fn corrupt_index_is_an_error_not_a_silent_fallback_when_explicit() {
    let (t, d) = project(false);
    let bad = t.path().join("bad.scip");
    std::fs::write(&bad, b"\xff\xff not protobuf").unwrap();
    let r = run_dead_code(
        &target(&t, &d),
        &DeadCodeParams {
            scip: ScipOpts {
                paths: vec![bad],
                disable: false,
            },
            ..Default::default()
        },
    );
    assert!(r.is_err());
}

#[test]
fn refs_are_precise_with_scip_and_ambiguous_without() {
    let (t, d) = project(false);
    let name_based = references(&target(&t, &d), "run", 0.0, 50).unwrap();
    assert!(
        name_based.iter().any(|h| h.target == "B.run"),
        "tier-0 links the call to both methods: {name_based:?}"
    );
    let (t2, d2) = project(true);
    let h = references_scip(&target(&t2, &d2), "run", 0.0, 50, &ScipOpts::default()).unwrap();
    assert!(h.iter().any(|x| x.target == "A.run" && x.source == "scip"));
    assert!(!h.iter().any(|x| x.target == "B.run"), "{h:?}");
}

#[test]
fn scip_status_reports_coverage() {
    let (t, d) = project(true);
    let st = plumbgraph_core::ops::scip_status(&target(&t, &d), &ScipOpts::default())
        .unwrap()
        .expect("index auto-detected");
    assert_eq!(st.documents, 1);
    assert_eq!(st.documents_matched, 1);
    assert_eq!(st.symbols_mapped, 5);
    assert_eq!(st.tools, vec!["scip-python 0.0-test".to_string()]);
    let (t, d) = project(false);
    assert!(
        plumbgraph_core::ops::scip_status(&target(&t, &d), &ScipOpts::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn files_not_in_the_index_keep_name_based_edges() {
    let (t, d) = project(true);
    // a second file the index does not know: calls B().run() by name
    std::fs::write(
        t.path().join("b.py"),
        "from a import B\n\ndef other():\n    B().run()\n\nother()\n",
    )
    .unwrap();
    let h = references_scip(&target(&t, &d), "run", 0.0, 50, &ScipOpts::default()).unwrap();
    assert!(
        h.iter()
            .any(|x| x.target == "B.run" && x.file == "b.py" && x.source != "scip"),
        "{h:?}"
    );
}

/// SCIP indexers cannot resolve calls on untyped receivers (plain JS, unannotated Python).
/// An occurrence the index does not report must keep its name-based edge instead of being
/// silently dropped, otherwise live code would be reported dead.
#[test]
fn reference_the_index_does_not_resolve_keeps_its_name_based_edge() {
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    let src = format!("{A_PY}\ndef other(b):\n    b.run()\nother(B())\n");
    std::fs::write(t.path().join("a.py"), &src).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    // same index as `write_index`, plus defs for `other`, but NO occurrence for `b.run()` (0-based line 11)
    let mut idx = Index::parse_from_bytes(&{
        write_index(&t.path().join("tmp.scip"));
        std::fs::read(t.path().join("tmp.scip")).unwrap()
    })
    .unwrap();
    std::fs::remove_file(t.path().join("tmp.scip")).unwrap();
    let so = "scip-python python pkg 0.1 `a`/other().";
    idx.documents[0].occurrences.push(occ(so, 10, 4, 9, true));
    idx.documents[0].occurrences.push(occ(so, 12, 0, 5, false));
    idx.documents[0].occurrences.push(occ(SB, 12, 6, 7, false));
    std::fs::write(t.path().join("index.scip"), idx.write_to_bytes().unwrap()).unwrap();
    let f = dead(&t, &d, ScipOpts::default());
    assert!(
        !f.iter().any(|x| x.symbol.as_deref() == Some("B.run")),
        "`b.run()` is unresolved by SCIP and must still count as a (name-based) use: {f:?}"
    );
}

/// Precision: the SCIP index disambiguates `A().run()` even though `run` also exists on B.
#[test]
fn resolved_occurrence_replaces_only_the_matching_name_edge() {
    let (t, d) = project(true);
    let h = references_scip(&target(&t, &d), "run", 0.0, 50, &ScipOpts::default()).unwrap();
    assert_eq!(h.iter().filter(|x| x.target == "A.run").count(), 1, "{h:?}");
    assert!(h.iter().all(|x| x.target != "B.run"));
}

/// rust-analyzer gives identical symbol strings to same-named items in different binary
/// targets of one package (it prints "Duplicate symbol"). A reference must bind to the
/// definition in its own document, never to whichever definition was seen first.
#[test]
fn duplicate_symbol_strings_bind_within_their_own_document() {
    let t = tempfile::tempdir().unwrap();
    let d = tempfile::tempdir().unwrap();
    // y.py calls helper() at module level; x.py (listed first in the index) defines the same
    // function but never calls it
    std::fs::write(t.path().join("x.py"), "def helper():\n    pass\n").unwrap();
    std::fs::write(
        t.path().join("y.py"),
        "def helper():\n    pass\n\nhelper()\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let sym = "scip-python python pkg 0.1 `m`/helper().";
    let mut dx = Document::new();
    dx.relative_path = "x.py".into();
    dx.occurrences = vec![occ(sym, 0, 4, 10, true)];
    let mut dy = Document::new();
    dy.relative_path = "y.py".into();
    dy.occurrences = vec![occ(sym, 0, 4, 10, true), occ(sym, 3, 0, 6, false)];
    let mut idx = Index::new();
    idx.documents = vec![dx, dy];
    std::fs::write(t.path().join("index.scip"), idx.write_to_bytes().unwrap()).unwrap();
    let f = dead(&t, &d, ScipOpts::default());
    let helpers: Vec<_> = f
        .iter()
        .filter(|x| x.symbol.as_deref() == Some("helper"))
        .map(|x| x.file.as_str())
        .collect();
    assert_eq!(
        helpers,
        vec!["x.py"],
        "y.py helper is called, x.py helper is not: {f:?}"
    );
}
