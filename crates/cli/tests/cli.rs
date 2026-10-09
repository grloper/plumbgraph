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
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 6);
    assert_eq!(
        lines[2]["result"]["structuredContent"]["data"]["findings"][0]["symbol"],
        "_dead"
    );
    assert_eq!(lines[3]["result"]["isError"], true);
}
