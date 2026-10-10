//! Unity messages on `partial` classes (v1.0.3). The Unity base may be declared in any part,
//! in another file; messages are merged per type. Fixture: `tests/fixtures/unity_partials`.
mod common;
use common::*;
use plumbgraph_core::ops::{run_dead_code, DeadCodeParams, DeadCodeResult, ScipOpts, Target};
use plumbgraph_core::Finding;

fn run() -> DeadCodeResult {
    let t = copy_fixture("unity_partials");
    let dbdir = tempfile::tempdir().unwrap();
    run_dead_code(
        &Target {
            root: t.path().to_path_buf(),
            db: Some(dbdir.path().join("i.db")),
        },
        &DeadCodeParams {
            min_confidence: 0.0,
            scip: ScipOpts::disabled(),
            ..Default::default()
        },
    )
    .unwrap()
}

fn find<'a>(f: &'a [Finding], q: &str) -> Option<&'a Finding> {
    f.iter().find(|x| x.symbol.as_deref() == Some(q))
}

fn symbols(f: &[Finding]) -> Vec<String> {
    let v: Vec<String> = f.iter().filter_map(|x| x.symbol.clone()).collect();
    // positive control: the `Class.Method` naming the absence checks rely on
    assert!(v.iter().any(|q| q == "Blob.BlobDeadHelper"), "{v:?}");
    v
}

#[test]
fn partial_messages_are_entry_points_when_any_part_declares_a_unity_base() {
    let s = symbols(&run().findings);
    for q in [
        "Blob.LateUpdate",
        "Blob.OnDisable",
        "AutoTest.FixedUpdate",
        // indirect base (Hero : CharacterBase : MonoBehaviour) resolved across the merge
        "Hero.LateUpdate",
        // editor partials
        "Window.OnGUI",
        "Window.Update",
        "Window.OnInspectorUpdate",
        "Window.OnFocus",
    ] {
        assert!(!s.contains(&q.to_string()), "false positive {q}: {s:?}");
    }
    // and the types themselves stay live
    for q in ["Blob", "AutoTest", "Hero", "Window"] {
        assert!(!s.contains(&q.to_string()), "false positive {q}: {s:?}");
    }
}

#[test]
fn unreferenced_private_helpers_on_a_monobehaviour_are_still_reported() {
    let f = run().findings;
    for q in [
        "Net.LocalPortString",
        "Net.NetExtraDeadHelper",
        "Blob.BlobDeadHelper",
    ] {
        let x = find(&f, q).unwrap_or_else(|| panic!("missing {q}"));
        // reportable under the default `--min-confidence 0.70`, with no Unity-specific discount
        assert!(x.confidence >= 0.70, "{x:?}");
        assert!(!x.fp_risks.iter().any(|r| r.contains("Unity")), "{x:?}");
    }
    assert!(find(&f, "Net.Update").is_none());
}

#[test]
fn plain_class_update_is_not_an_engine_callback() {
    let f = run().findings;
    for q in [
        "TasterState.Update",
        "TasterState.UpdateScore",
        "TasterState.ApplyUpdate",
        "Mover.RefreshUpdateState",
        "PlainPart.Update",
        // plain base class resolved in the index: not a component, even under Assets/
        "DerivedState.Update",
    ] {
        assert!(find(&f, q).is_some(), "missing {q}");
    }
    // same-file MonoBehaviour.Update is still suppressed
    assert!(find(&f, "Mover.Update").is_none());
}

#[test]
fn unresolved_host_type_is_medium_with_a_reason_never_high() {
    let f = run().findings;
    let m = find(&f, "Mystery.Update").expect("unresolved host: reported, not silently live");
    assert_eq!(m.level.as_str(), "medium", "{m:?}");
    assert!(
        m.fp_risks
            .iter()
            .any(|r| r.contains("unity-message-name-but-host-type-unresolved")),
        "{:?}",
        m.fp_risks
    );
    // HIGH is only allowed for exact Unity message names on types proven non-Unity
    let proven_non_unity = [
        "TasterState.Update",
        "PlainPart.Update",
        "DerivedState.Update",
    ];
    for x in &f {
        let q = x.symbol.clone().unwrap_or_default();
        let name = q.rsplit('.').next().unwrap_or("");
        if ["Update", "LateUpdate", "FixedUpdate", "OnDisable", "OnGUI"].contains(&name)
            && x.level.as_str() == "high"
        {
            assert!(proven_non_unity.contains(&q.as_str()), "HIGH on {q}: {x:?}");
        }
    }
}

#[test]
fn partial_merge_leaves_a_debug_note() {
    let r = run();
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("unity-message@partial-merged") && n.contains("Blob.LateUpdate")),
        "{:?}",
        r.notes
    );
}

#[test]
fn editor_bases_and_attributes() {
    let s = symbols(&run().findings);
    for q in [
        "Inspector.OnInspectorGUI",
        "Inspector.OnSceneGUI",
        "Wizard.OnWizardCreate",
        "Wizard.OnWizardUpdate",
        "Wizard.OnWizardOtherButton",
        "Startup.Do",
        "Startup.OnLoad",
        "Startup.Boot",
        "Startup",
    ] {
        assert!(!s.contains(&q.to_string()), "false positive {q}: {s:?}");
    }
    // real dead code next to them, and static helpers unrelated to the attributes
    for q in [
        "Inspector.InspectorDeadHelper",
        "Wizard.WizardDeadHelper",
        "Startup.UnrelatedStaticHelper",
    ] {
        assert!(s.contains(&q.to_string()), "missing {q}: {s:?}");
    }
}

#[test]
fn unity_pack_footer_notes_when_indexing_under_assets() {
    let r = run();
    let l = r.limitations.join("\n");
    assert!(l.contains("Unity pack active. Scenes/prefabs/UnityEvents unread. Partial MonoBehaviour messages are entry points when any part declares a Unity base."), "{l}");
    assert!(l.contains("Do not mass-delete HIGH findings in Assets/ without confirming engine callbacks and serialization."), "{l}");
}
