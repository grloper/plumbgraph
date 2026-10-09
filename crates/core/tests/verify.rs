//! `plumb verify` (baseline diffing), provider detection/running, rule-engine parsing.
use plumbgraph_core::diagnostics::Format;
use plumbgraph_core::ops::{DiagInput, ScipOpts, Target};
use plumbgraph_core::providers;
use plumbgraph_core::rules;
use plumbgraph_core::verify::{run_verify, VerifyParams};
use plumbgraph_core::Level;
use std::path::{Path, PathBuf};
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
        "app.py",
        "def used():\n    return 1\n\ndef dead_one():\n    return 2\n\nprint(used())\n",
    );
    git(r, &["init", "-q"]);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "init"]);
    t
}

fn params() -> VerifyParams {
    VerifyParams {
        scip: ScipOpts::disabled(),
        ..Default::default()
    }
}

fn target(t: &tempfile::TempDir) -> Target {
    Target {
        root: t.path().to_path_buf(),
        db: None,
    }
}

#[test]
fn verify_baseline_only_new_findings_fail() {
    let t = project();
    let r = t.path();
    let p = params();

    // 1. no baseline: the existing dead function is a new finding -> fail
    let rep = run_verify(&target(&t), &p).unwrap();
    assert_eq!(rep.verdict, "fail");
    assert!(rep
        .new
        .iter()
        .any(|f| f.symbol.as_deref() == Some("dead_one")));

    // 2. write the baseline
    let rep = run_verify(
        &target(&t),
        &VerifyParams {
            update_baseline: true,
            ..params()
        },
    )
    .unwrap();
    assert!(rep.baseline_updated);
    assert!(r.join("plumb-baseline.json").is_file());

    // 3. same code: passes, finding is baselined
    let rep = run_verify(&target(&t), &p).unwrap();
    assert_eq!(rep.verdict, "pass", "{:?}", rep.new);
    assert_eq!(rep.baselined, 1);
    assert!(rep.new.is_empty());

    // 4. moving the code (line shift) does not create a "new" finding
    write(
        r,
        "app.py",
        "# comment\n# comment\ndef used():\n    return 1\n\ndef dead_one():\n    return 2\n\nprint(used())\n",
    );
    let rep = run_verify(&target(&t), &p).unwrap();
    assert_eq!(rep.verdict, "pass", "{:?}", rep.new);

    // 5. a genuinely new dead function fails, and only that one is reported
    write(
        r,
        "app.py",
        "def used():\n    return 1\n\ndef dead_one():\n    return 2\n\ndef dead_two():\n    return 3\n\nprint(used())\n",
    );
    let rep = run_verify(&target(&t), &p).unwrap();
    assert_eq!(rep.verdict, "fail");
    let syms: Vec<_> = rep.new.iter().filter_map(|f| f.symbol.clone()).collect();
    assert_eq!(syms, vec!["dead_two".to_string()]);
    assert_eq!(rep.baselined, 1);

    // 6. removing the baselined dead code is reported as fixed
    write(r, "app.py", "def used():\n    return 1\n\nprint(used())\n");
    let rep = run_verify(&target(&t), &p).unwrap();
    assert_eq!(rep.verdict, "pass");
    assert_eq!(rep.fixed, 1);
}

#[test]
fn verify_merges_saved_diagnostics_and_baselines_them() {
    let t = project();
    let ruff = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diag/ruff.json"),
    )
    .unwrap();
    write(
        t.path(),
        "app.py",
        "def used():\n    return 1\n\nprint(used())\n",
    );
    let mk = |update: bool| VerifyParams {
        diag_inputs: vec![DiagInput {
            format: Format::Ruff,
            label: "ruff.json".into(),
            text: ruff.clone(),
        }],
        update_baseline: update,
        ..params()
    };
    let rep = run_verify(&target(&t), &mk(false)).unwrap();
    let n = rep
        .new
        .iter()
        .filter(|f| f.category == "diagnostic")
        .count();
    assert!(
        n > 0,
        "diagnostics from the saved ruff output must be merged: {:?}",
        rep.new
    );
    assert_eq!(rep.verdict, "fail");
    assert!(rep
        .new
        .iter()
        .all(|f| !f.source.is_empty() && f.confidence > 0.0));
    run_verify(&target(&t), &mk(true)).unwrap();
    let rep = run_verify(&target(&t), &mk(false)).unwrap();
    assert_eq!(rep.verdict, "pass", "{:?}", rep.new);
    assert_eq!(rep.baselined, n);
}

#[test]
fn verify_flags_test_weakening_against_base() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(r, "m.py", "def f():\n    return 1\n\nprint(f())\n");
    write(
        r,
        "test_m.py",
        "from m import f\n\ndef test_f():\n    assert f() == 1\n",
    );
    git(r, &["init", "-q"]);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "init"]);
    write(
        r,
        "test_m.py",
        "import pytest\nfrom m import f\n\n@pytest.mark.skip\ndef test_f():\n    assert f() == 1\n",
    );
    let rep = run_verify(&target(&t), &params()).unwrap();
    assert_eq!(rep.verdict, "fail");
    assert!(
        rep.new.iter().any(|f| f.category == "weakening"),
        "{:?}",
        rep.new
    );
    // diff-based findings are never absorbed by a baseline
    run_verify(
        &target(&t),
        &VerifyParams {
            update_baseline: true,
            ..params()
        },
    )
    .unwrap();
    let rep = run_verify(&target(&t), &params()).unwrap();
    assert!(rep.new.iter().any(|f| f.category == "weakening"));
}

#[test]
fn verify_fail_on_threshold_and_execution_is_opt_in() {
    let t = project();
    let rep = run_verify(
        &target(&t),
        &VerifyParams {
            fail_on: Level::High,
            ..params()
        },
    )
    .unwrap();
    // dead_one is a medium-confidence finding: not enough to fail at `high`
    assert_eq!(rep.verdict, "pass");
    assert!(!rep.executed, "nothing may run without opt-in");
    assert!(rep
        .steps
        .iter()
        .any(|s| s.name == "diagnostics" && s.status == "skipped"));
}

#[cfg(unix)]
fn fake_bin(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn providers_are_detected_on_a_search_path_and_ran_only_on_request() {
    let bin = tempfile::tempdir().unwrap();
    // a fake scip-python that writes a valid (empty) SCIP index to --output
    fake_bin(
        bin.path(),
        "scip-python",
        "out=''; while [ $# -gt 0 ]; do if [ \"$1\" = --output ]; then out=$2; fi; shift; done; : > \"$out\"; echo ran > \"$out.marker\"",
    );
    let t = project();
    let path: Vec<PathBuf> = vec![bin.path().to_path_buf()];
    let det = providers::detect(t.path(), &path);
    let sp = det.iter().find(|d| d.id == "scip-python").unwrap();
    assert!(sp.available && sp.relevant, "{sp:?}");
    assert!(sp.runs_project_code);
    let ra = det.iter().find(|d| d.id == "rust-analyzer-scip").unwrap();
    assert!(!ra.relevant, "no Rust files here");
    // detection executed nothing
    assert!(!t
        .path()
        .join(".plumbgraph/scip/scip-python.scip.marker")
        .exists());

    let runs = providers::run_scip_indexers(
        t.path(),
        &["scip-python".to_string()],
        &path,
        std::time::Duration::from_secs(30),
    );
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "ran", "{:?}", runs[0]);
    assert!(t
        .path()
        .join(".plumbgraph/scip/scip-python.scip.marker")
        .exists());
    // unknown ids are refused, not guessed
    let runs = providers::run_scip_indexers(
        t.path(),
        &["rm -rf".to_string()],
        &path,
        std::time::Duration::from_secs(5),
    );
    assert_eq!(runs[0].status, "refused");
    // discovered by the SCIP overlay
    assert!(plumbgraph_core::scip::discover(t.path())
        .iter()
        .any(|p| p.ends_with("scip-python.scip")));
}

#[test]
fn semgrep_and_astgrep_output_is_normalised() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rules");
    let root = Path::new("/proj");
    let sg = std::fs::read_to_string(dir.join("semgrep.json")).unwrap();
    let f = rules::parse_semgrep(&sg, root).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].source, "semgrep");
    assert_eq!(f[0].rule, "no-shell-true");
    assert_eq!((f[0].file.as_str(), f[0].line), ("a.py", 3));
    assert!(f[0].confidence > 0.0 && f[0].confidence <= 1.0);
    let ag = std::fs::read_to_string(dir.join("astgrep.jsonl")).unwrap();
    let f = rules::parse_astgrep(&ag, root).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].source, "ast-grep");
    assert_eq!(
        (f[0].file.as_str(), f[0].line),
        ("e.py", 1),
        "ast-grep lines are 0-based"
    );
    assert!(rules::parse_semgrep("not json", root).is_err());
    // a path escaping the project is dropped
    let evil = r#"{"results":[{"check_id":"x","path":"../../etc/passwd","start":{"line":1,"col":1},"extra":{"message":"m","severity":"ERROR"}}]}"#;
    assert!(rules::parse_semgrep(evil, root).unwrap().is_empty());
}

#[test]
fn init_writes_instructions_and_mcp_config_without_clobbering() {
    use plumbgraph_core::init::{init, BEGIN};
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    git(r, &["init", "-q"]);
    write(r, "AGENTS.md", "# Mine\n\nkeep this\n");
    write(
        r,
        ".cursor/mcp.json",
        "{\"mcpServers\":{\"other\":{\"command\":\"x\"}}}",
    );
    write(r, "CLAUDE.md", "# claude notes\n");
    let rep = init(r, &[], false).unwrap();
    let agents = std::fs::read_to_string(r.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# Mine\n\nkeep this\n"), "{agents}");
    assert!(agents.contains(BEGIN));
    assert!(std::fs::read_to_string(r.join("CLAUDE.md"))
        .unwrap()
        .contains(BEGIN));
    let cursor: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".cursor/mcp.json")).unwrap())
            .unwrap();
    assert_eq!(cursor["mcpServers"]["other"]["command"], "x");
    assert_eq!(cursor["mcpServers"]["plumbgraph"]["args"][0], "mcp");
    // the server is never registered with --allow-exec
    assert!(!std::fs::read_to_string(r.join(".cursor/mcp.json"))
        .unwrap()
        .contains("allow-exec"));
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["plumbgraph"]["command"], "plumb");
    assert!(rep
        .snippets
        .iter()
        .any(|s| s.contains("mcp_servers.plumbgraph")));
    assert!(std::fs::read_to_string(r.join(".gitignore"))
        .unwrap()
        .contains(".plumbgraph/"));
    // idempotent
    let again = init(r, &[], false).unwrap();
    assert!(again.written.is_empty(), "{again:?}");
    assert_eq!(
        std::fs::read_to_string(r.join("AGENTS.md")).unwrap(),
        agents
    );
    // dry run writes nothing
    let t2 = tempfile::tempdir().unwrap();
    let rep = init(t2.path(), &["claude".to_string()], true).unwrap();
    assert!(!t2.path().join("AGENTS.md").exists() && !rep.written.is_empty());
    // invalid JSON is an error, not overwritten
    write(t2.path(), ".mcp.json", "{ nope");
    assert!(init(t2.path(), &["claude".to_string()], false).is_err());
    assert_eq!(
        std::fs::read_to_string(t2.path().join(".mcp.json")).unwrap(),
        "{ nope"
    );
}
