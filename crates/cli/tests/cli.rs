use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn plumb() -> Command {
    Command::new(env!("CARGO_BIN_EXE_plumb"))
}

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
    write(t.path(), "pkg/a.py", "import requests\n\ndef used():\n    return 1\n\ndef _dead():\n    return 2\n\nprint(used())\n");
    write(t.path(), "requirements.txt", "requests\n");
    t
}

#[test]
fn version_and_both_binaries() {
    let out = plumb().arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("plumb"));
    let out = Command::new(env!("CARGO_BIN_EXE_plumbgraph"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn index_then_dead_code_json() {
    let t = project();
    let out = plumb().args(["index"]).arg(t.path()).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("symbols 2"));
    let out = plumb()
        .args(["--json", "dead-code"])
        .arg(t.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let f = &v["findings"][0];
    assert_eq!(f["symbol"], "_dead");
    assert_eq!(f["level"], "high");
    assert_eq!(f["source"], "t0-treesitter");
    assert!(f["confidence"].as_f64().unwrap() >= 0.9);
    assert!(f["evidence"].as_array().unwrap().len() >= 2);
}

#[test]
fn fail_on_sets_exit_code() {
    let t = project();
    let ok = plumb().args(["dead-code"]).arg(t.path()).status().unwrap();
    assert!(ok.success());
    let st = plumb()
        .args(["dead-code", "--fail-on", "high"])
        .arg(t.path())
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(1));
}

#[test]
fn check_deps_offline_text() {
    let t = project();
    write(t.path(), "pkg/b.py", "import notdeclaredpkg\n");
    let out = plumb()
        .args(["check-deps", "--offline"])
        .arg(t.path())
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("notdeclaredpkg"), "{s}");
    assert!(s.contains("offline"));
    assert!(!s.contains("nonexistent"));
}

#[test]
fn bad_path_is_exit_2() {
    let out = plumb()
        .args(["index", "/definitely/not/a/dir"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error"));
}

#[test]
fn weakening_in_git_repo() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    git(d, &["init", "-q", "-b", "main"]);
    write(
        d,
        "tests/test_a.py",
        "def test_one():\n    assert 1 == 1\n\ndef test_two():\n    assert 2 == 2\n",
    );
    git(d, &["add", "."]);
    git(d, &["commit", "-q", "-m", "init"]);
    write(
        d,
        "tests/test_a.py",
        "import pytest\n\n@pytest.mark.skip\ndef test_one():\n    assert 1 == 1\n",
    );
    let out = plumb()
        .args([
            "--json",
            "weakening",
            "--base",
            "HEAD",
            "--fail-on",
            "high",
            "--repo",
        ])
        .arg(d)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules: Vec<&str> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(
        rules.contains(&"deleted-test") && rules.contains(&"added-skip"),
        "{rules:?}"
    );
}

#[test]
fn mcp_stdio_roundtrip() {
    let t = project();
    let mut child = plumb()
        .args(["mcp", "--root"])
        .arg(t.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"t","version":"0"}}}}}}"#).unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"dead_code","arguments":{{}}}}}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"dead_code","arguments":{{"path":"../.."}}}}}}"#).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4, "notifications get no response");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "plumbgraph");
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 13);
    assert_eq!(
        lines[2]["result"]["structuredContent"]["data"]["findings"][0]["symbol"],
        "_dead"
    );
    assert_eq!(lines[3]["result"]["isError"], true);
}

// ---------------------------------------------------------------- v0.2: diagnostics + SCIP

fn diag_fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../core/tests/fixtures/diag")
        .join(name)
}

fn json_of(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn diagnostics_ingest_saved_output_does_not_execute_anything() {
    let t = project();
    let from = format!("ruff-json:{}", diag_fixture("ruff.json").display());
    let out = plumb()
        .args(["--json", "diagnostics"])
        .arg(t.path())
        .args(["--from", &from])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json_of(&out);
    assert_eq!(v["executed"], false);
    assert_eq!(v["findings"].as_array().unwrap().len(), 4);
    assert_eq!(v["findings"][0]["source"], "lint:ruff");
    assert!(v["findings"][0]["confidence"].as_f64().unwrap() > 0.0);
}

#[test]
fn diagnostics_text_output_and_fail_on() {
    let t = project();
    let from = format!("tsc:{}", diag_fixture("tsc.txt").display());
    let out = plumb()
        .arg("diagnostics")
        .arg(t.path())
        .args(["--from", &from, "--fail-on", "high"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("a.ts:1:7") && s.contains("TS2322") && s.contains("[typecheck:tsc]"),
        "{s}"
    );
}

#[test]
fn diagnostics_without_inputs_is_a_usage_error() {
    let t = project();
    let out = plumb().arg("diagnostics").arg(t.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let e = String::from_utf8_lossy(&out.stderr);
    assert!(e.contains("--from") && e.contains("--run"), "{e}");
    let out = plumb()
        .arg("diagnostics")
        .arg(t.path())
        .args(["--from", "bogus:file"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn diagnostics_run_skips_missing_tools_instead_of_failing() {
    let t = project();
    write(t.path(), "tsconfig.json", "{}");
    let out = plumb()
        .args(["--json", "diagnostics"])
        .arg(t.path())
        .args(["--run", "--tool", "tsc"])
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json_of(&out);
    assert_eq!(v["tools"][0]["tool"], "tsc");
    assert_eq!(v["tools"][0]["status"], "skipped");
    assert_eq!(v["executed"], false);
}

#[test]
fn diagnostics_via_lsp_client() {
    let t = project();
    let server = format!("python3 {}", diag_fixture("fake_lsp.py").display());
    let out = plumb()
        .args(["--json", "diagnostics"])
        .arg(t.path())
        .args([
            "--lsp",
            &server,
            "--lsp-file",
            "pkg/a.py",
            "--timeout-secs",
            "10",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json_of(&out);
    assert_eq!(v["executed"], true);
    assert_eq!(v["findings"][0]["file"], "pkg/a.py");
    assert!(v["findings"][0]["source"]
        .as_str()
        .unwrap()
        .starts_with("lsp:"));
}

fn scip_index(path: &Path) {
    use protobuf::Message;
    use scip::types::{Document, Index, Occurrence, SymbolRole};
    let sym = |n: &str| format!("scip-python python pkg 0.1 `a`/{n}");
    let occ = |s: &str, line: i32, def: bool| {
        let mut o = Occurrence::new();
        o.symbol = s.into();
        o.range = vec![line, 4, 8];
        if def {
            o.symbol_roles = protobuf::Enum::value(&SymbolRole::Definition);
        }
        o
    };
    let mut d = Document::new();
    d.relative_path = "pkg/a.py".into();
    // pkg/a.py: line1 import, line3 used(), line6 _dead(), line9 print(used())
    d.occurrences = vec![
        occ(&sym("used()."), 2, true),
        occ(&sym("_dead()."), 5, true),
        occ(&sym("used()."), 8, false),
    ];
    let mut i = Index::new();
    i.documents = vec![d];
    std::fs::write(path, i.write_to_bytes().unwrap()).unwrap();
}

#[test]
fn dead_code_with_scip_reports_source_scip_and_no_scip_falls_back() {
    let t = project();
    std::thread::sleep(std::time::Duration::from_millis(20));
    scip_index(&t.path().join("index.scip"));
    let run = |extra: &[&str]| {
        let out = plumb()
            .args(["--json", "dead-code"])
            .arg(t.path())
            .args(extra)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        json_of(&out)
    };
    let v = run(&[]);
    assert_eq!(v["scip"]["symbols_mapped"], 2);
    let f = v["findings"].as_array().unwrap();
    assert!(
        f.iter()
            .any(|x| x["symbol"] == "_dead" && x["source"] == "scip"),
        "{f:?}"
    );
    let v = run(&["--no-scip"]);
    assert!(v["scip"].is_null());
    assert!(v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["source"] != "scip"));
    // explicit but broken index is an error, not a silent fallback
    write(t.path(), "broken.scip", "not protobuf \u{ff}");
    let out = plumb()
        .arg("dead-code")
        .arg(t.path())
        .args(["--scip", t.path().join("broken.scip").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn scip_status_command() {
    let t = project();
    let out = plumb().args(["scip"]).arg(t.path()).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no SCIP index"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    scip_index(&t.path().join("index.scip"));
    let out = plumb()
        .args(["--json", "scip"])
        .arg(t.path())
        .output()
        .unwrap();
    let v = json_of(&out);
    assert_eq!(v["documents_matched"], 1);
}

#[test]
fn diagnostics_lsp_files_are_relative_to_the_project_even_with_a_relative_path_argument() {
    let t = project();
    let server = format!("python3 {}", diag_fixture("fake_lsp.py").display());
    let out = plumb()
        .current_dir(t.path().parent().unwrap())
        .args(["--json", "diagnostics"])
        .arg(t.path().file_name().unwrap())
        .args([
            "--lsp",
            &server,
            "--lsp-file",
            "pkg/a.py",
            "--timeout-secs",
            "10",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json_of(&out)["findings"][0]["file"], "pkg/a.py");
}

// ---------------------------------------------------------------- v1: map + impact

#[test]
fn map_and_impact_commands() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "lib.py", "def core():\n    return 1\n");
    write(
        t.path(),
        "app.py",
        "from lib import core\n\ndef run():\n    return core()\n",
    );
    let out = plumb()
        .args(["map"])
        .arg(t.path())
        .args(["--tokens", "300"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("lib.py") && text.contains("def core()"),
        "{text}"
    );
    let out = plumb()
        .args(["--json", "impact", "core", "--path"])
        .arg(t.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["affected"][0]["name"], "run");
    let out = plumb()
        .args(["impact", "nope", "--path"])
        .arg(t.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn map_budget_is_an_alias_of_tokens() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "lib.py", "def core():\n    return 1\n");
    let run = |args: &[&str]| {
        plumb()
            .arg("map")
            .arg(t.path())
            .args(args)
            .output()
            .unwrap()
    };
    let by_budget = run(&["--budget", "300"]);
    assert!(
        by_budget.status.success(),
        "{}",
        String::from_utf8_lossy(&by_budget.stderr)
    );
    let by_tokens = run(&["--tokens", "300"]);
    assert_eq!(by_budget.stdout, by_tokens.stdout, "same flag, same output");
    assert!(String::from_utf8_lossy(&by_budget.stdout).contains("budget 300"));
    // giving the budget twice under both names is a usage error, not a silent override
    assert_eq!(
        run(&["--budget", "300", "--tokens", "500"]).status.code(),
        Some(2)
    );
    // the alias is visible in --help
    let help = plumb().args(["map", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(
        help.contains("--tokens") && help.contains("--budget"),
        "{help}"
    );
}

#[test]
fn init_dry_run_shows_the_gitignore_lines() {
    let t = project();
    git(t.path(), &["init", "-q"]);
    let out = plumb()
        .args(["init"])
        .arg(t.path())
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("would add to") && text.contains(".plumbgraph/*"),
        "{text}"
    );
    assert!(text.contains("!.plumbgraph/allow.toml"), "{text}");
    assert!(
        !t.path().join(".gitignore").exists(),
        "dry run writes nothing"
    );
    let out = plumb()
        .args(["init"])
        .arg(t.path())
        .args(["--no-gitignore"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!t.path().join(".gitignore").exists());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--no-gitignore"));
}

// ---------------------------------------------------------------- v1: verify, doctor, init

#[test]
fn verify_exit_codes_and_baseline_flow() {
    let t = project();
    git(t.path(), &["init", "-q"]);
    git(t.path(), &["add", "-A"]);
    git(t.path(), &["commit", "-q", "-m", "i"]);
    let run = |extra: &[&str]| {
        plumb()
            .args(["verify"])
            .arg(t.path())
            .args(extra)
            .output()
            .unwrap()
    };
    let out = run(&["--no-such-flag"]);
    assert_eq!(out.status.code(), Some(2), "unknown flag is a usage error");
    let out = run(&[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("nothing was executed"));
    let out = run(&["--update-baseline"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(t.path().join("plumb-baseline.json").is_file());
    let out = run(&[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("VERIFY: PASS")
            || String::from_utf8_lossy(&out.stdout).contains("verify: PASS")
    );
    let out = plumb()
        .args(["--json", "verify"])
        .arg(t.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["verdict"], "pass");
    assert_eq!(v["baselined"], 1);
}

#[test]
fn doctor_and_init_commands() {
    let t = project();
    let out = plumb()
        .args(["--json", "doctor"])
        .arg(t.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "scip-python" && p["relevant"] == true));
    let out = plumb()
        .args(["init"])
        .arg(t.path())
        .args(["--agent", "claude", "--dry-run"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !t.path().join("AGENTS.md").exists(),
        "dry run writes nothing"
    );
    let out = plumb()
        .args(["init"])
        .arg(t.path())
        .args(["--agent", "claude"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(t.path().join("AGENTS.md").is_file() && t.path().join(".mcp.json").is_file());
}

fn many_dead() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let mut src = String::from("def main():\n    return 1\n\nmain()\n");
    for i in 0..30 {
        src.push_str(&format!("\ndef _dead_{i}():\n    return {i}\n"));
    }
    write(t.path(), "pkg/a.py", &src);
    t
}

#[test]
fn dead_code_output_is_capped_with_summary() {
    let t = many_dead();
    let out = plumb()
        .args(["--json", "dead-code", "--max-findings", "5"])
        .arg(t.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 5);
    assert_eq!(v["summary"]["total"].as_u64().unwrap(), 30);
    assert_eq!(v["truncated"]["shown"], 5);
    assert_eq!(v["truncated"]["total"], 30);
    // text mode says so too
    let out = plumb()
        .args(["dead-code", "--max-findings", "5"])
        .arg(t.path())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("25 more finding(s) not shown"));
    // 0 = unlimited, and no "truncated" key
    let out = plumb()
        .args(["--json", "dead-code", "--max-findings", "0"])
        .arg(t.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 30);
    assert!(v.get("truncated").is_none());
}

#[test]
fn exit_code_uses_all_findings_even_when_output_is_capped() {
    let t = many_dead();
    let out = plumb()
        .args(["dead-code", "--max-findings", "1", "--fail-on", "high"])
        .arg(t.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn closed_stdout_pipe_does_not_panic() {
    let t = many_dead();
    let mut c = plumb()
        .args(["--json", "dead-code", "--max-findings", "0"])
        .arg(t.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(c.stdout.take()); // reader goes away before the first write
    let out = c.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
    assert!(!err.contains("failed printing"), "{err}");
}

#[test]
fn verify_output_is_capped_but_verdict_counts_everything() {
    let t = many_dead();
    let out = plumb()
        .args(["--json", "verify", "--max-findings", "5"])
        .arg(t.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["new"].as_array().unwrap().len(), 5);
    assert_eq!(v["truncated"]["total"], 30);
    assert_eq!(v["total_findings"], 30);
    assert_eq!(v["verdict"], "fail");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn mcp_startup_line_on_stderr_has_version_and_pid() {
    let t = project();
    let mut child = plumb()
        .args(["mcp", "--root"])
        .arg(t.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    drop(child.stdin.take()); // EOF: the server exits after printing its startup line
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("pid={pid}")), "{err}");
    assert!(
        err.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))),
        "{err}"
    );
}

#[test]
fn dead_code_help_links_the_unity_pack_limitations() {
    let out = plumb().args(["dead-code", "--help"]).output().unwrap();
    let h = String::from_utf8_lossy(&out.stdout);
    assert!(h.contains("docs/PACKS.md#unity-c"), "{h}");
}

#[test]
fn dead_code_prints_unity_footer_under_assets() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "Assets/A.cs",
        "using UnityEngine;\nclass A : MonoBehaviour { void Update() { } void Dead() { } }\n",
    );
    let out = plumb().args(["dead-code"]).arg(t.path()).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("note: Unity pack active. Scenes/prefabs/UnityEvents unread. Partial MonoBehaviour messages are entry points when any part declares a Unity base."), "{s}");
    assert!(s.contains("note: Do not mass-delete HIGH findings in Assets/ without confirming engine callbacks and serialization."), "{s}");
    // and not for projects without Assets/
    let p = project();
    let out = plumb().args(["dead-code"]).arg(p.path()).output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("Unity pack active"));
}

#[test]
fn dead_code_verbose_prints_unity_partial_merge_notes() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "Assets/B.cs",
        "partial class B { void LateUpdate() { } }\n",
    );
    write(
        t.path(),
        "Assets/B.Core.cs",
        "using UnityEngine;\npartial class B : MonoBehaviour { }\n",
    );
    let out = plumb()
        .args(["dead-code", "--verbose"])
        .arg(t.path())
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("debug: unity-message@partial-merged B.LateUpdate"),
        "{s}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!s.contains("B.LateUpdate` appears unused"), "{s}");
    // without --verbose the note is not printed
    let out = plumb().args(["dead-code"]).arg(t.path()).output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("partial-merged"));
}
