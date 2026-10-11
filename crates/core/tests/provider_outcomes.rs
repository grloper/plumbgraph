//! Fail-closed classification of real subprocess outcomes, without global PATH mutation.
use plumbgraph_core::diagnostics::{parse, run_tools, Format, ToolSpec};
use std::path::Path;
use std::time::Duration;

fn run(format: Format, stdout: &str, code: i32) -> plumbgraph_core::diagnostics::DiagReport {
    let root = tempfile::tempdir().unwrap();
    run_tools(
        root.path(),
        &[ToolSpec {
            name: "fake-provider".into(),
            format,
            program: "python3".into(),
            args: vec![
                "-c".into(),
                "import sys; sys.stdout.write(sys.argv[1]); sys.exit(int(sys.argv[2]))".into(),
                stdout.into(),
                code.to_string(),
            ],
        }],
        Duration::from_secs(5),
    )
}

#[test]
fn nonzero_empty_and_unrecognized_diagnostic_outputs_fail() {
    for (format, output, code) in [
        (Format::CargoJson, "build failed\n", 101),
        (
            Format::CargoJson,
            "{\"reason\":\"build-finished\",\"success\":false}\n",
            101,
        ),
        (Format::Tsc, "error TS5058: missing tsconfig.json\n", 1),
        (Format::Tsc, "", 2),
        (Format::Ruff, "[]", 1),
        (Format::Pyright, "{\"generalDiagnostics\":[]}", 1),
    ] {
        let rep = run(format, output, code);
        assert!(rep.executed, "python3 must be installed for fake providers");
        assert_eq!(rep.tools[0].status, "failed", "{format:?}: {output}");
    }
}

#[test]
fn successful_empty_diagnostics_remain_valid() {
    for (format, output) in [
        (Format::CargoJson, ""),
        (Format::Tsc, ""),
        (Format::Ruff, "[]"),
        (Format::Pyright, "{\"generalDiagnostics\":[]}"),
    ] {
        assert_eq!(run(format, output, 0).tools[0].status, "ran");
    }
}

#[test]
fn valid_findings_exit_is_accepted_but_crash_exit_is_not() {
    let output = "a.ts(1,1): error TS1234: test finding\n";
    for (code, status) in [(1, "ran"), (2, "ran"), (3, "failed"), (137, "failed")] {
        let rep = run(Format::Tsc, output, code);
        assert_eq!(rep.tools[0].status, status);
        assert_eq!(rep.tools[0].findings, 1);
        assert_eq!(rep.findings[0].source, "typecheck:tsc");
        assert!(rep.findings[0].confidence > 0.0);
    }
}

#[test]
fn malformed_json_entries_are_not_silently_dropped() {
    assert!(parse(Format::Ruff, "[{}]", Path::new("/project")).is_err());
    assert!(parse(
        Format::Pyright,
        "{\"generalDiagnostics\":[{}]}",
        Path::new("/project")
    )
    .is_err());
}

#[test]
fn failed_process_with_only_outside_root_findings_is_not_clean() {
    let rep = run(
        Format::Tsc,
        "../../elsewhere.ts(1,1): error TS1234: bad\n",
        2,
    );
    assert_eq!(rep.tools[0].status, "failed");
    assert_eq!(rep.dropped_outside_root, 1);
    assert!(rep.findings.is_empty());
}

#[test]
fn real_diagnostic_payloads_keep_their_normal_nonzero_exit_semantics() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diag");
    for (format, name, code) in [
        (Format::CargoJson, "cargo.jsonl", 101),
        (Format::Ruff, "ruff.json", 1),
        (Format::Pyright, "pyright.json", 1),
    ] {
        // Real fixtures use /workspace/samples/proj absolute paths. Relativize them
        // to the temporary fake provider project, preserving the report fields.
        let output = std::fs::read_to_string(fixture.join(name))
            .unwrap()
            .replace("/workspace/samples/proj/", "");
        let rep = run(format, &output, code);
        assert_eq!(
            rep.tools[0].status, "ran",
            "{format:?}: {:?}",
            rep.tools[0].note
        );
        assert!(!rep.findings.is_empty());
        assert!(rep
            .findings
            .iter()
            .all(|f| !f.source.is_empty() && f.confidence > 0.0));
    }
}

#[test]
#[cfg(unix)]
fn signal_termination_is_not_a_success_even_with_findings() {
    let root = tempfile::tempdir().unwrap();
    let rep = run_tools(root.path(), &[ToolSpec {
        name: "killed-provider".into(),
        format: Format::Tsc,
        program: "python3".into(),
        args: vec!["-c".into(), "import os, signal; print('a.ts(1,1): error TS1234: finding', flush=True); os.kill(os.getpid(), signal.SIGKILL)".into()],
    }], Duration::from_secs(5));
    assert_eq!(rep.tools[0].status, "failed");
    assert_eq!(rep.findings.len(), 1);
}

#[test]
fn malformed_saved_text_diagnostics_are_rejected_on_ingest() {
    for format in [Format::CargoJson, Format::Tsc] {
        for output in ["garbage", "{}", "{broken json", "permission denied"] {
            assert!(
                parse(format, output, Path::new("/project")).is_err(),
                "{format:?}: {output}"
            );
            assert_eq!(run(format, output, 0).tools[0].status, "failed");
        }
    }
    assert!(parse(
        Format::CargoJson,
        r#"{"reason":"compiler-message","message":{}}"#,
        Path::new("/project")
    )
    .is_err());
    assert!(parse(
        Format::CargoJson,
        r#"{"reason":"build-finished","success":false}"#,
        Path::new("/project")
    )
    .is_err());
    assert!(parse(
        Format::Tsc,
        "error TS5058: missing tsconfig.json",
        Path::new("/project")
    )
    .is_err());
}

#[test]
fn recognized_metadata_only_saved_outputs_are_valid() {
    for output in [
        r#"{"reason":"build-finished","success":true}"#,
        r#"{"reason":"compiler-artifact","target":{"name":"example"}}"#,
        r#"{"reason":"build-script-executed","package_id":"example"}"#,
    ] {
        assert!(parse(Format::CargoJson, output, Path::new("/project"))
            .unwrap()
            .0
            .is_empty());
    }
    assert!(
        parse(Format::Tsc, "Found 0 errors.\n", Path::new("/project"))
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn partial_cargo_json_and_missing_success_are_not_clean_evidence() {
    for text in [
        "{\"reason\":\"compiler-artifact\"}\n{\"reason\":\"compiler-message\",\"message\":",
        "{\"reason\":\"build-finished\"}",
        "{\"reason\":\"build-finished\",\"success\":\"true\"}",
    ] {
        assert!(parse(Format::CargoJson, text, Path::new("/project")).is_err());
    }
}

#[test]
fn astgrep_overflowing_positions_are_rejected_without_panicking() {
    for value in [4294967295_u64, u64::MAX] {
        let record = serde_json::json!({"file":"a.py", "ruleId":"x", "severity":"error", "message":"test", "range":{"start":{"line":value,"column":value}}});
        assert!(
            plumbgraph_core::rules::parse_astgrep(&record.to_string(), Path::new("/project"))
                .is_err()
        );
    }
}
