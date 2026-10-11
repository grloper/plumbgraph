//! Pinned, offline end-to-end gate contract. No project code or providers execute.
use plumbgraph_core::diagnostics::Format;
use plumbgraph_core::ops::{DiagInput, ScipOpts, Target};
use plumbgraph_core::verify::{
    run_verify, VerifyParams, VerifyPolicy, VerifyReport, BASELINE_FILE,
};
use plumbgraph_core::Level;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/gate")
            .join(name),
    )
    .unwrap()
}
fn put(root: &Path, path: &str, data: impl AsRef<[u8]>) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Offline corpus")
        .env("GIT_AUTHOR_EMAIL", "corpus@example.invalid")
        .env("GIT_COMMITTER_NAME", "Offline corpus")
        .env("GIT_COMMITTER_EMAIL", "corpus@example.invalid")
        .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn project() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for name in ["app.py", "test_app.py", "package.json"] {
        put(t.path(), name, fixture(name));
    }
    put(t.path(), BASELINE_FILE, fixture("empty-baseline.json"));
    git(t.path(), &["init", "-q"]);
    commit(t.path());
    t
}
fn commit(root: &Path) -> String {
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "Pinned corpus state",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}
fn params() -> VerifyParams {
    VerifyParams {
        scip: ScipOpts::disabled(),
        ..Default::default()
    }
}
fn run(root: &Path, p: VerifyParams) -> VerifyReport {
    let report = run_verify(
        &Target {
            root: root.into(),
            db: None,
        },
        &p,
    )
    .unwrap();
    assert!(!report.executed, "offline corpus must execute no providers");
    assert_eq!(report.schema_version, 2);
    assert!(report
        .new
        .iter()
        .all(|f| !f.source.is_empty() && f.confidence > 0.0));
    report
}
fn expected(case: &str) -> String {
    let manifest: serde_json::Value = serde_json::from_slice(&fixture("manifest.json")).unwrap();
    manifest["cases"][case].as_str().unwrap().to_owned()
}
fn diagnostic(name: &str) -> DiagInput {
    DiagInput {
        format: Format::Ruff,
        label: name.into(),
        text: String::from_utf8(fixture(name)).unwrap(),
    }
}

#[test]
fn pinned_clean_and_legitimate_refactor_pass() {
    for (file, replacement, case) in [
        ("app.py", "clean.py", "clean-change"),
        ("test_app.py", "refactored.py", "legitimate-refactor"),
    ] {
        let t = project();
        put(t.path(), file, fixture(replacement));
        let r = run(t.path(), params());
        assert_eq!(r.verdict, expected(case), "{r:?}");
        assert!(!r.new.iter().any(|f| f.category == "weakening"));
        assert!(r.index.is_some());
        assert!(r.scip.is_none());
    }
}

#[test]
fn pinned_fake_dependency_fails_offline_despite_high_dead_code_threshold() {
    let t = project();
    put(t.path(), "injected.js", fixture("fake_dependency.js"));
    let r = run(
        t.path(),
        VerifyParams {
            fail_on: Level::High,
            ..params()
        },
    );
    assert_eq!(r.verdict, expected("fake-dependency"), "{r:?}");
    assert!(r.new.iter().any(|f| f.rule == "undeclared-dependency"));
    assert!(!r.new.iter().any(|f| f.rule.contains("nonexistent")));
    assert!(r.steps.iter().any(
        |s| s.name == "check-deps" && s.note.as_deref().unwrap_or_default().contains("offline")
    ));
}

#[test]
fn pinned_weakening_and_test_deletion_fail_and_are_counted() {
    for case in ["assertion-weakening", "test-deletion"] {
        let t = project();
        if case == "test-deletion" {
            std::fs::remove_file(t.path().join("test_app.py")).unwrap();
        } else {
            put(t.path(), "test_app.py", fixture("weakened.py"));
        }
        let r = run(
            t.path(),
            VerifyParams {
                fail_on: Level::High,
                ..params()
            },
        );
        assert_eq!(r.verdict, expected(case), "{r:?}");
        let count = r.new.iter().filter(|f| f.category == "weakening").count();
        assert!(count > 0);
        assert_eq!(r.total_findings, r.new.len() + r.baselined);
        assert!(r.total_findings >= count);
    }
}

#[test]
fn dead_code_is_advisory_until_explicitly_enforced() {
    let t = project();
    put(t.path(), "app.py", fixture("dead.py"));
    let r = run(t.path(), params());
    assert_eq!(r.verdict, "pass");
    assert!(r.new.iter().any(|f| f.category == "dead"));
    let r = run(
        t.path(),
        VerifyParams {
            policy: VerifyPolicy {
                enforce_dead_code: true,
                ..Default::default()
            },
            ..params()
        },
    );
    assert_eq!(r.verdict, "fail");
}

#[test]
fn library_and_unity_limitations_survive_verify() {
    let t = project();
    put(t.path(), "api.js", fixture("library.js"));
    let app = run(t.path(), params());
    assert!(
        app.limitations.iter().any(|s| s.contains("public API")),
        "{:?}",
        app.limitations
    );
    let lib = run(
        t.path(),
        VerifyParams {
            library_mode: true,
            ..params()
        },
    );
    assert!(!lib
        .new
        .iter()
        .any(|f| f.file == "api.js" && f.category == "dead"));
    put(t.path(), "Assets/Player.cs", fixture("Player.cs"));
    let unity = run(t.path(), params());
    assert!(unity
        .limitations
        .iter()
        .any(|s| s.contains("Scenes/prefabs/UnityEvents unread")));
    assert!(unity
        .limitations
        .iter()
        .any(|s| s.contains("Do not mass-delete")));
    assert!(unity
        .limitations
        .iter()
        .any(|s| s.contains("reflection or macro expansion")));
}

#[test]
fn hostile_worktree_baseline_cannot_hide_new_dependencies() {
    let t = project();
    let trusted = git(t.path(), &["rev-parse", "HEAD"]);
    put(t.path(), "injected.js", fixture("fake_dependency.js"));
    let maintenance = run(
        t.path(),
        VerifyParams {
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(maintenance.verdict, "baseline-updated");
    assert_eq!(
        run(t.path(), params()).verdict,
        "pass",
        "fixture must demonstrate local baseline absorption"
    );
    let r = run(
        t.path(),
        VerifyParams {
            trusted_baseline: Some(trusted.clone()),
            ..params()
        },
    );
    assert_eq!(r.verdict, expected("hostile-baseline-edit"));
    assert_eq!(r.trusted_baseline.as_deref(), Some(trusted.as_str()));
    assert!(r.new.iter().any(|f| f.category == "deps"));
}

#[test]
fn trusted_commit_also_pins_weakening_base() {
    let t = project();
    let trusted = git(t.path(), &["rev-parse", "HEAD"]);
    put(t.path(), "test_app.py", fixture("weakened.py"));
    commit(t.path());
    assert_eq!(run(t.path(), params()).verdict, "pass");
    let r = run(
        t.path(),
        VerifyParams {
            base: "HEAD".into(),
            trusted_baseline: Some(trusted),
            ..params()
        },
    );
    assert_eq!(r.verdict, "fail");
    assert!(r.new.iter().any(|f| f.category == "weakening"));
}

#[test]
fn invalid_git_base_is_incomplete_and_blocks_maintenance() {
    let t = project();
    let original = std::fs::read(t.path().join(BASELINE_FILE)).unwrap();
    let r = run(
        t.path(),
        VerifyParams {
            base: "nonexistent-corpus-revision".into(),
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(r.verdict, expected("invalid-git-base"));
    assert!(!r.baseline_updated);
    assert_eq!(
        std::fs::read(t.path().join(BASELINE_FILE)).unwrap(),
        original
    );
    assert!(r
        .steps
        .iter()
        .any(|s| s.name == "test-weakening" && s.status == "failed"));
}

#[test]
fn malformed_saved_inputs_are_incomplete_and_cannot_write_baseline() {
    for case in ["malformed-diagnostic", "malformed-scip"] {
        let t = project();
        let original = std::fs::read(t.path().join(BASELINE_FILE)).unwrap();
        let mut p = VerifyParams {
            update_baseline: true,
            ..params()
        };
        if case == "malformed-diagnostic" {
            p.diag_inputs.push(diagnostic("malformed-diagnostic.json"));
        } else {
            put(t.path(), "broken.scip", fixture("malformed.scip"));
            p.scip = ScipOpts {
                paths: vec![t.path().join("broken.scip")],
                disable: false,
            };
        }
        let r = run(t.path(), p);
        assert_eq!(r.verdict, expected(case), "{r:?}");
        assert!(!r.baseline_updated);
        assert_eq!(
            std::fs::read(t.path().join(BASELINE_FILE)).unwrap(),
            original
        );
    }
}

#[test]
fn maintenance_has_distinct_verdict_and_never_absorbs_weakening() {
    let t = project();
    let r = run(
        t.path(),
        VerifyParams {
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(r.verdict, expected("baseline-maintenance"));
    assert!(r.baseline_updated);
    let original = std::fs::read(t.path().join(BASELINE_FILE)).unwrap();
    put(t.path(), "test_app.py", fixture("weakened.py"));
    let r = run(
        t.path(),
        VerifyParams {
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(r.verdict, "fail");
    assert!(!r.baseline_updated);
    assert_eq!(
        std::fs::read(t.path().join(BASELINE_FILE)).unwrap(),
        original
    );
}

#[test]
fn optional_provider_baseline_is_retained_when_not_observed() {
    let t = project();
    let r = run(
        t.path(),
        VerifyParams {
            diag_inputs: vec![diagnostic("diagnostic.json")],
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(r.verdict, "baseline-updated");
    assert!(r.new.iter().any(|f| f.category == "diagnostic"));
    let original = std::fs::read(t.path().join(BASELINE_FILE)).unwrap();
    let r = run(
        t.path(),
        VerifyParams {
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(r.baseline_unobserved, 1);
    assert_eq!(r.fixed, 0);
    assert_eq!(
        std::fs::read(t.path().join(BASELINE_FILE)).unwrap(),
        original
    );
    let r = run(
        t.path(),
        VerifyParams {
            diag_inputs: vec![diagnostic("diagnostic.json")],
            ..params()
        },
    );
    assert_eq!(r.verdict, "pass");
    assert_eq!(r.baselined, 1);
}

#[test]
fn diagnostic_threshold_is_independent_of_dead_code_threshold() {
    let t = project();
    let r = run(
        t.path(),
        VerifyParams {
            diag_inputs: vec![diagnostic("diagnostic.json")],
            fail_on: Level::High,
            ..params()
        },
    );
    assert_eq!(r.verdict, "fail");
    assert!(r.new.iter().any(|f| f.category == "diagnostic"));
}

#[test]
fn explicit_baseline_errors_and_implicit_absence_are_distinct() {
    let t = project();
    let target = Target {
        root: t.path().into(),
        db: None,
    };
    std::fs::remove_file(t.path().join(BASELINE_FILE)).unwrap();
    assert_eq!(run(t.path(), params()).verdict, "pass");
    assert!(run_verify(
        &target,
        &VerifyParams {
            baseline: Some(t.path().join("missing.json")),
            ..params()
        }
    )
    .is_err());
    for (fixture_name, expected_error) in [
        ("malformed-baseline.json", "invalid baseline JSON"),
        ("unsupported-baseline.json", "unsupported baseline version"),
    ] {
        put(t.path(), BASELINE_FILE, fixture(fixture_name));
        let error = run_verify(&target, &params()).unwrap_err();
        assert!(format!("{error:#}").contains(expected_error), "{error:#}");
    }
}

#[test]
fn trusted_mode_rejects_mutable_refs_maintenance_and_overrides() {
    let t = project();
    let trusted = git(t.path(), &["rev-parse", "HEAD"]);
    let target = Target {
        root: t.path().into(),
        db: None,
    };
    for p in [
        VerifyParams {
            trusted_baseline: Some("HEAD".into()),
            ..params()
        },
        VerifyParams {
            trusted_baseline: Some("0".repeat(40)),
            ..params()
        },
        VerifyParams {
            trusted_baseline: Some(trusted.clone()),
            update_baseline: true,
            ..params()
        },
        VerifyParams {
            trusted_baseline: Some(trusted),
            baseline: Some(PathBuf::from(t.path()).join(BASELINE_FILE)),
            ..params()
        },
    ] {
        assert!(run_verify(&target, &p).is_err());
    }
}

#[test]
fn trusted_allowlist_cannot_be_added_modified_deleted_or_untracked() {
    for change in ["added", "modified", "deleted", "untracked"] {
        let t = project();
        if matches!(change, "modified" | "deleted") {
            put(t.path(), ".plumbgraph/allow.toml", b"# trusted allowlist\n");
            commit(t.path());
        }
        let trusted = git(t.path(), &["rev-parse", "HEAD"]);
        if change == "deleted" {
            std::fs::remove_file(t.path().join(".plumbgraph/allow.toml")).unwrap();
        } else {
            put(
                t.path(),
                ".plumbgraph/allow.toml",
                b"# hostile allowlist change\n",
            );
        }
        if change == "added" {
            git(t.path(), &["add", ".plumbgraph/allow.toml"]);
        }
        let error = run_verify(
            &Target {
                root: t.path().into(),
                db: None,
            },
            &VerifyParams {
                trusted_baseline: Some(trusted),
                ..params()
            },
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("allow"),
            "{change}: {error:#}"
        );
    }
}

#[cfg(unix)]
#[test]
fn trusted_allowlist_symlink_is_rejected() {
    let t = project();
    let trusted = git(t.path(), &["rev-parse", "HEAD"]);
    put(t.path(), "innocent.toml", b"# identical empty allowlist\n");
    std::fs::create_dir_all(t.path().join(".plumbgraph")).unwrap();
    std::os::unix::fs::symlink(
        t.path().join("innocent.toml"),
        t.path().join(".plumbgraph/allow.toml"),
    )
    .unwrap();
    assert!(run_verify(
        &Target {
            root: t.path().into(),
            db: None
        },
        &VerifyParams {
            trusted_baseline: Some(trusted),
            ..params()
        }
    )
    .is_err());
}

#[test]
fn explicit_missing_baseline_can_be_created_relative_to_project() {
    let t = project();
    let r = run(
        t.path(),
        VerifyParams {
            baseline: Some(PathBuf::from("reviewed-baseline.json")),
            update_baseline: true,
            ..params()
        },
    );
    assert_eq!(r.verdict, "baseline-updated");
    assert!(t.path().join("reviewed-baseline.json").is_file());
    let r = run(
        t.path(),
        VerifyParams {
            baseline: Some(PathBuf::from("reviewed-baseline.json")),
            ..params()
        },
    );
    assert_eq!(r.verdict, "pass");
}

#[test]
fn incomplete_analyzer_does_not_mark_dead_baseline_entries_fixed() {
    let t = project();
    put(t.path(), "app.py", fixture("dead.py"));
    let r = run(
        t.path(),
        VerifyParams {
            update_baseline: true,
            ..params()
        },
    );
    assert!(r.new.iter().any(|f| f.category == "dead"));
    put(t.path(), "broken.scip", fixture("malformed.scip"));
    let r = run(
        t.path(),
        VerifyParams {
            scip: ScipOpts {
                paths: vec![t.path().join("broken.scip")],
                disable: false,
            },
            ..params()
        },
    );
    assert_eq!(r.verdict, "incomplete");
    assert_eq!(r.fixed, 0);
    assert!(r.baseline_unobserved > 0);
}

#[test]
fn per_check_thresholds_only_control_their_own_category() {
    let t = project();
    put(t.path(), "injected.js", fixture("fake_dependency.js"));
    for (dependencies, diagnostics, verdict) in [
        (Level::High, Level::Medium, "fail"),
        (Level::Medium, Level::High, "fail"),
        (Level::High, Level::High, "pass"),
    ] {
        let r = run(
            t.path(),
            VerifyParams {
                diag_inputs: vec![diagnostic("diagnostic.json")],
                fail_on: Level::Low,
                policy: VerifyPolicy {
                    dependencies,
                    diagnostics,
                    ..Default::default()
                },
                ..params()
            },
        );
        assert_eq!(r.verdict, verdict, "{r:?}");
        assert!(r.new.iter().any(|f| f.category == "deps"));
        assert!(r.new.iter().any(|f| f.category == "diagnostic"));
    }
}

#[test]
fn trusted_baseline_ignores_malformed_worktree_baseline_content() {
    let t = project();
    let trusted = git(t.path(), &["rev-parse", "HEAD"]);
    put(t.path(), BASELINE_FILE, fixture("malformed-baseline.json"));
    let r = run(
        t.path(),
        VerifyParams {
            trusted_baseline: Some(trusted),
            ..params()
        },
    );
    assert_eq!(r.verdict, "pass");
    assert!(!r.baseline_updated);
    assert_eq!(
        std::fs::read(t.path().join(BASELINE_FILE)).unwrap(),
        fixture("malformed-baseline.json")
    );
}

#[cfg(unix)]
#[test]
fn autodiscovered_scip_symlink_outside_project_is_incomplete() {
    let t = project();
    let outside = tempfile::tempdir().unwrap();
    // Empty protobuf is valid. Failure must be confinement, not parsing.
    put(outside.path(), "index.scip", b"");
    std::os::unix::fs::symlink(
        outside.path().join("index.scip"),
        t.path().join("index.scip"),
    )
    .unwrap();
    let r = run(
        t.path(),
        VerifyParams {
            scip: ScipOpts::default(),
            ..params()
        },
    );
    assert_eq!(r.verdict, "incomplete");
    assert!(r
        .steps
        .iter()
        .any(|s| s.name == "dead-code" && s.status == "failed"));
    assert!(!r.baseline_updated);
}

#[test]
fn partial_cargo_evidence_cannot_pass_or_replace_baseline() {
    let t = project();
    let before = std::fs::read(t.path().join(BASELINE_FILE)).unwrap();
    for text in [
        "{\"reason\":\"compiler-artifact\"}\n{\"reason\":\"compiler-message\",\"message\":",
        "{\"reason\":\"build-finished\"}",
    ] {
        let r = run(
            t.path(),
            VerifyParams {
                update_baseline: true,
                diag_inputs: vec![DiagInput {
                    format: Format::CargoJson,
                    label: "truncated.json".into(),
                    text: text.into(),
                }],
                ..params()
            },
        );
        assert_eq!(r.verdict, "incomplete");
        assert!(!r.baseline_updated);
        assert_eq!(std::fs::read(t.path().join(BASELINE_FILE)).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn verify_refuses_cache_and_policy_symlink_escapes_before_indexing() {
    use std::os::unix::fs::symlink;
    for rel in [
        ".plumbgraph",
        ".plumbgraph/index.db",
        ".plumbgraph/allow.toml",
    ] {
        let t = project();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim");
        std::fs::write(&victim, "unchanged").unwrap();
        let link = t.path().join(rel);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(
            if rel == ".plumbgraph" {
                outside.path()
            } else {
                &victim
            },
            &link,
        )
        .unwrap();
        assert!(run_verify(
            &Target {
                root: t.path().into(),
                db: None
            },
            &params()
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "unchanged");
    }
}
