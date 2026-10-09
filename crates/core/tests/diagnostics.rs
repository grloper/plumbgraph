//! Diagnostics: normalisation of real compiler/linter output, execution and LSP.
mod common;
use common::fixture_dir;
use plumbgraph_core::diagnostics::{builtin_tools, parse, run_tools, Format, ToolSpec};
use plumbgraph_core::lsp;
use plumbgraph_core::{Level, Severity};
use std::path::Path;
use std::time::Duration;

fn fx(name: &str) -> String {
    std::fs::read_to_string(fixture_dir("diag").join(name)).unwrap()
}

const ROOT: &str = "/workspace/samples/proj";

#[test]
fn format_names() {
    assert_eq!("cargo-json".parse::<Format>().unwrap(), Format::CargoJson);
    assert_eq!("ruff-json".parse::<Format>().unwrap(), Format::Ruff);
    assert_eq!("pyright-json".parse::<Format>().unwrap(), Format::Pyright);
    assert_eq!("tsc".parse::<Format>().unwrap(), Format::Tsc);
    assert!("nope".parse::<Format>().is_err());
}

#[test]
fn cargo_errors_from_real_rustc_output() {
    let (f, dropped) = parse(Format::CargoJson, &fx("cargo.jsonl"), Path::new(ROOT)).unwrap();
    assert_eq!(dropped, 0);
    // the two `failure-note` summary lines are not diagnostics
    assert_eq!(f.len(), 2, "{f:#?}");
    let e = f.iter().find(|x| x.rule == "E0308").unwrap();
    assert_eq!(
        (e.file.as_str(), e.line, e.column),
        ("src/main.rs", 3, Some(18))
    );
    assert_eq!(e.severity, Severity::Error);
    assert_eq!(e.source, "compiler:rustc");
    assert_eq!(e.category, "diagnostic");
    assert!(e.confidence >= 0.95 && e.level == Level::High);
    assert!(e.message.contains("mismatched types"));
}

#[test]
fn clippy_lints_are_labelled_and_less_certain() {
    let (f, _) = parse(Format::CargoJson, &fx("clippy.jsonl"), Path::new(ROOT)).unwrap();
    assert_eq!(f.len(), 4, "{f:#?}");
    let lz = f.iter().find(|x| x.rule == "clippy::len_zero").unwrap();
    assert_eq!(lz.source, "lint:clippy");
    assert_eq!(lz.severity, Severity::Warning);
    let dead = f.iter().find(|x| x.rule == "dead_code").unwrap();
    assert_eq!(dead.source, "compiler:rustc");
    assert!(lz.confidence < dead.confidence);
}

#[test]
fn ruff_json() {
    let (f, _) = parse(Format::Ruff, &fx("ruff.json"), Path::new(ROOT)).unwrap();
    assert_eq!(f.len(), 4);
    let u = f.iter().find(|x| x.rule == "F821").unwrap();
    assert_eq!((u.file.as_str(), u.line, u.column), ("app.py", 7, Some(7)));
    assert_eq!(u.source, "lint:ruff");
    assert!(u.message.contains("undefined_name"));
}

#[test]
fn ruff_absolute_paths_are_made_relative_and_foreign_ones_dropped() {
    let text = r#"[{"code":"F401","message":"m","filename":"/workspace/samples/proj/pkg/a.py","location":{"row":3,"column":1}},
                   {"code":"F401","message":"m","filename":"/etc/passwd","location":{"row":1,"column":1}},
                   {"code":"F401","message":"m","filename":"../../outside.py","location":{"row":1,"column":1}}]"#;
    let (f, dropped) = parse(Format::Ruff, text, Path::new(ROOT)).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].file, "pkg/a.py");
    assert_eq!(dropped, 2);
}

#[test]
fn pyright_json_is_zero_based_in_the_input() {
    let (f, _) = parse(Format::Pyright, &fx("pyright.json"), Path::new(ROOT)).unwrap();
    assert_eq!(f.len(), 2);
    let r = f
        .iter()
        .find(|x| x.rule == "reportUndefinedVariable")
        .unwrap();
    assert_eq!((r.line, r.column), (7, Some(7)));
    assert_eq!(r.source, "typecheck:pyright");
    assert_eq!(r.severity, Severity::Error);
}

#[test]
fn tsc_text_output() {
    let (f, _) = parse(Format::Tsc, &fx("tsc.txt"), Path::new(ROOT)).unwrap();
    assert_eq!(f.len(), 3, "{f:#?}");
    assert_eq!(f[0].rule, "TS2322");
    assert_eq!(
        (f[0].file.as_str(), f[0].line, f[0].column),
        ("a.ts", 1, Some(7))
    );
    assert_eq!(f[2].rule, "TS2304");
    assert_eq!(f[2].source, "typecheck:tsc");
}

#[test]
fn garbage_is_an_error_but_noise_lines_are_tolerated() {
    assert!(parse(Format::Ruff, "this is not json", Path::new(ROOT)).is_err());
    assert!(parse(Format::Pyright, "{\"nope\":1}", Path::new(ROOT)).is_err());
    // cargo emits non-JSON lines (e.g. build script output) in some setups
    let mixed = format!("warning: something\n{}", fx("cargo.jsonl"));
    assert_eq!(
        parse(Format::CargoJson, &mixed, Path::new(ROOT))
            .unwrap()
            .0
            .len(),
        2
    );
    assert_eq!(parse(Format::Tsc, "", Path::new(ROOT)).unwrap().0.len(), 0);
}

#[test]
fn messages_are_sanitised() {
    let text = "a.ts(1,1): error TS1: bad \u{1b}[31mthing\u{7}\n";
    let (f, _) = parse(Format::Tsc, text, Path::new(ROOT)).unwrap();
    assert!(!f[0].message.contains('\u{1b}') && !f[0].message.contains('\u{7}'));
}

fn script_tool(dir: &Path, name: &str, body: &str, fmt: Format) -> ToolSpec {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ToolSpec {
        name: name.into(),
        format: fmt,
        program: p.display().to_string(),
        args: vec![],
    }
}

#[test]
fn run_tools_executes_captures_and_reports_status() {
    let t = tempfile::tempdir().unwrap();
    let ok = script_tool(
        t.path(),
        "fake-tsc",
        "echo \"a.ts(1,2): error TS9: boom\"; exit 2",
        Format::Tsc,
    );
    let missing = ToolSpec {
        name: "ghost".into(),
        format: Format::Ruff,
        program: "/nonexistent/plumb-ghost".into(),
        args: vec![],
    };
    let slow = script_tool(t.path(), "slow", "sleep 5", Format::Tsc);
    let crash = script_tool(t.path(), "crash", "echo oops >&2; exit 3", Format::Ruff);
    let r = run_tools(
        t.path(),
        &[ok, missing, slow, crash],
        Duration::from_millis(700),
    );
    assert!(r.executed);
    assert_eq!(r.findings.len(), 1);
    let st = |n: &str| r.tools.iter().find(|x| x.tool == n).unwrap().status.clone();
    assert_eq!(st("fake-tsc"), "ran");
    assert_eq!(st("ghost"), "skipped");
    assert_eq!(st("slow"), "timeout");
    assert_eq!(st("crash"), "failed");
}

#[test]
fn builtin_tools_follow_project_files() {
    let t = tempfile::tempdir().unwrap();
    assert!(builtin_tools(t.path()).is_empty());
    std::fs::write(t.path().join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(t.path().join("tsconfig.json"), "{}").unwrap();
    std::fs::write(t.path().join("pyproject.toml"), "").unwrap();
    let names: Vec<_> = builtin_tools(t.path())
        .into_iter()
        .map(|s| s.name)
        .collect();
    for n in ["cargo-check", "clippy", "tsc", "ruff", "pyright"] {
        assert!(names.contains(&n.to_string()), "{n} in {names:?}");
    }
}

fn have_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn lsp_client_collects_publish_diagnostics() {
    assert!(
        have_python(),
        "python3 is required for the fake LSP server test"
    );
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.py"), "x = 1\nbad line\n").unwrap();
    let server = vec![
        "python3".to_string(),
        fixture_dir("diag")
            .join("fake_lsp.py")
            .display()
            .to_string(),
    ];
    let r = lsp::collect(
        &server,
        t.path(),
        &[t.path().join("a.py")],
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(r.executed);
    assert_eq!(r.findings.len(), 2, "{:#?}", r.findings);
    let e = r.findings.iter().find(|f| f.rule == "E1").unwrap();
    assert_eq!((e.file.as_str(), e.line, e.column), ("a.py", 2, Some(5)));
    assert_eq!(e.severity, Severity::Error);
    assert!(e.source.starts_with("lsp:"), "{}", e.source);
    assert!(!e.message.contains('\u{1b}'));
    assert!(
        e.message.contains("python"),
        "languageId passed: {}",
        e.message
    );
    let hint = r
        .findings
        .iter()
        .find(|f| f.message == "just a hint")
        .unwrap();
    assert_eq!(hint.severity, Severity::Info);
}

#[test]
fn lsp_missing_server_and_file_outside_root_are_errors() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.py"), "x").unwrap();
    assert!(lsp::collect(
        &["/nonexistent/plumb-lsp".to_string()],
        t.path(),
        &[t.path().join("a.py")],
        Duration::from_secs(2)
    )
    .is_err());
    let server = vec![
        "python3".to_string(),
        fixture_dir("diag")
            .join("fake_lsp.py")
            .display()
            .to_string(),
    ];
    assert!(lsp::collect(
        &server,
        t.path(),
        &[Path::new("/etc/passwd").to_path_buf()],
        Duration::from_secs(2)
    )
    .is_err());
}

#[test]
fn lsp_waits_for_work_done_progress_before_trusting_an_empty_publish() {
    assert!(have_python());
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.py"), "x = 1\nbad line\n").unwrap();
    let server = vec![
        "python3".to_string(),
        fixture_dir("diag")
            .join("fake_lsp.py")
            .display()
            .to_string(),
        "--slow".to_string(),
    ];
    let r = lsp::collect(
        &server,
        t.path(),
        &[t.path().join("a.py")],
        Duration::from_secs(15),
    )
    .unwrap();
    assert_eq!(
        r.findings.len(),
        2,
        "diagnostics published after the loading progress ended: {:#?}",
        r.findings
    );
}

#[test]
fn lsp_waits_for_server_status_quiescence() {
    assert!(have_python());
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.py"), "x = 1\nbad line\n").unwrap();
    let server = vec![
        "python3".to_string(),
        fixture_dir("diag")
            .join("fake_lsp.py")
            .display()
            .to_string(),
        "--slow-status".to_string(),
    ];
    let r = lsp::collect(
        &server,
        t.path(),
        &[t.path().join("a.py")],
        Duration::from_secs(15),
    )
    .unwrap();
    assert_eq!(r.findings.len(), 2, "{:#?}", r.findings);
}

#[test]
fn lsp_source_name_skips_launcher_words() {
    assert!(have_python());
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a.py"), "x = 1\nbad line\n").unwrap();
    let server = vec![
        "python3".to_string(),
        fixture_dir("diag")
            .join("fake_lsp.py")
            .display()
            .to_string(),
    ];
    let r = lsp::collect(
        &server,
        t.path(),
        &[t.path().join("a.py")],
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(
        r.findings.iter().all(|f| f.source == "lsp:fake_lsp.py"),
        "{:?}",
        r.findings[0].source
    );
}
