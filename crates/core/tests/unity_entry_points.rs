//! Unity C#: engine-invoked methods (MonoBehaviour messages, Unity attributes) are entry
//! points; public component methods are only low-confidence leads (UnityEvent bindings live in
//! scenes/prefabs, which are not indexed). Fixture: `tests/fixtures/unity`.
mod common;
use common::*;
use plumbgraph_core::ops::{run_dead_code, DeadCodeParams, ScipOpts, Target};
use plumbgraph_core::Finding;

fn all_findings() -> Vec<Finding> {
    let t = copy_fixture("unity");
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
    .findings
}

fn symbols(f: &[Finding]) -> Vec<String> {
    let mut v: Vec<String> = f.iter().filter_map(|x| x.symbol.clone()).collect();
    v.sort();
    // positive control: proves the `Class.Method` naming the absence checks below rely on
    // (C# namespaces are not part of the qualified name), so they cannot pass vacuously
    assert!(v.iter().any(|q| q == "Player.NeverCalled"), "{v:?}");
    v
}

const MESSAGES: &[&str] = &[
    "Awake",
    "Start",
    "Update",
    "LateUpdate",
    "FixedUpdate",
    "OnEnable",
    "OnDisable",
    "OnDestroy",
    "OnCollisionEnter",
    "OnCollisionStay",
    "OnCollisionExit",
    "OnTriggerEnter",
    "OnTriggerStay",
    "OnTriggerExit",
    "OnCollisionEnter2D",
    "OnCollisionStay2D",
    "OnCollisionExit2D",
    "OnTriggerEnter2D",
    "OnTriggerStay2D",
    "OnTriggerExit2D",
    "OnGUI",
    "OnValidate",
    "Reset",
];

#[test]
fn monobehaviour_messages_are_entry_points() {
    let s = symbols(&all_findings());
    for m in MESSAGES {
        let q = format!("Player.{m}");
        assert!(!s.contains(&q), "false positive {q}: {s:?}");
    }
    // the class itself is live (its messages are), and so is what they call
    for q in ["Player", "Player.Init", "Player.Move"] {
        assert!(!s.contains(&q.to_string()), "false positive {q}: {s:?}");
    }
}

#[test]
fn scriptableobject_networkbehaviour_and_indirect_subclasses_under_assets() {
    let s = symbols(&all_findings());
    for q in [
        "GameSettings.OnEnable",
        "GameSettings.OnValidate",
        "NetPlayer.Start",
        "NetPlayer.Update",
        "Character.Awake",
        // Enemy : Character (a MonoBehaviour declared in another file), under Assets/
        "Enemy.Update",
        "Enemy.OnTriggerEnter",
        "Enemy.Chase",
        // EditorWindow in a Unity file, incl. the IHasCustomMenu callback
        "SceneBrowser.OnGUI",
        "SceneBrowser.AddItemsToMenu",
        // Unity callback interfaces, also on classes that are not components
        "SaveData.OnBeforeSerialize",
        "SaveData.OnAfterDeserialize",
        "ClickTarget.OnPointerClick",
        "ClickTarget.OnDrag",
    ] {
        assert!(!s.contains(&q.to_string()), "false positive {q}: {s:?}");
    }
}

#[test]
fn unity_attributes_are_entry_points() {
    let s = symbols(&all_findings());
    for q in [
        "BuildTools.Bake",
        "BuildTools.ClearCache",
        "BuildTools.Boot",
        "BuildTools.OnEditorLoad",
        "StartupHook",
        "Player.RespawnFromInspector",
    ] {
        assert!(!s.contains(&q.to_string()), "false positive {q}: {s:?}");
    }
}

#[test]
fn real_dead_code_in_unity_projects_is_still_reported() {
    let s = symbols(&all_findings());
    for q in [
        "Player.NeverCalled",
        "Character.UnusedCharacterHelper",
        "Enemy.EnemyDeadHelper",
        "GameSettings.StaleSettingsHelper",
        "NetPlayer.NetDeadHelper",
        "BuildTools.OrphanTool",
        // same names, but no Unity base type: not called by the engine
        "MathUtil.Update",
        "MathUtil.Start",
        "Worker.Start",
        "Worker.Update",
        // `[MenuItem]` outside a Unity project is just an attribute
        "Worker.Menu",
        "SceneBrowser.UnusedBrowserHelper",
        // under Assets/, but only interfaces as bases: cannot be a MonoBehaviour
        "Pooled.Update",
        // implementing a callback interface does not make other methods entry points
        "SaveData.UnusedSaveHelper",
        // a class merely named `Editor` outside a Unity file
        "TextEditor.OnGUI",
        "TextEditor.OnInspectorGUI",
    ] {
        assert!(s.contains(&q.to_string()), "missing {q}: {s:?}");
    }
}

#[test]
fn public_component_methods_are_low_confidence_with_a_unityevent_risk() {
    let f = all_findings();
    let jump = f
        .iter()
        .find(|x| x.symbol.as_deref() == Some("Player.Jump"))
        .expect("Jump is still reported, as a lead");
    assert_eq!(jump.level.as_str(), "low", "{jump:?}");
    assert!(
        jump.fp_risks.iter().any(|r| r.contains("UnityEvent")),
        "{:?}",
        jump.fp_risks
    );
    // private helpers in the same class do not get that risk
    let never = f
        .iter()
        .find(|x| x.symbol.as_deref() == Some("Player.NeverCalled"))
        .unwrap();
    assert!(!never.fp_risks.iter().any(|r| r.contains("UnityEvent")));
    assert_ne!(never.level.as_str(), "low", "{never:?}");
}

#[test]
fn serialize_field_fields_are_never_reported() {
    // C# fields are not extracted as symbols, so `[SerializeField]` fields cannot be
    // reported; this guards that if field extraction is ever added.
    let s = symbols(&all_findings());
    assert!(
        !s.iter()
            .any(|q| q.ends_with(".speed") || q.ends_with(".onDied")),
        "{s:?}"
    );
}

#[test]
fn outside_assets_unity_context_comes_from_code_not_comments_or_strings() {
    let t = tempfile::tempdir().unwrap();
    let w = |rel: &str, c: &str| {
        let p = t.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    };
    // a real `using UnityEditor;` (e.g. an embedded package outside Assets/)
    w(
        "Tools/Real.cs",
        "using UnityEditor;\nstatic class RealTools {\n    [MenuItem(\"X/Y\")]\n    static void RealMenu() { }\n}\n",
    );
    // a fully qualified attribute, no using
    w(
        "Tools/Qualified.cs",
        "static class QualifiedTools {\n    [global::UnityEditor.MenuItem(\"X/Z\")]\n    static void QualifiedMenu() { }\n}\n",
    );
    // only a comment, a string and a look-alike namespace mention Unity
    w(
        "Tools/Fake.cs",
        "// using UnityEditor;\nusing UnityEditorTools;\nstatic class FakeTools {\n    const string S = \"UnityEngine.Debug\";\n    [MenuItem]\n    static void FakeMenu() { }\n}\n",
    );
    let r = run_dead_code(
        &Target {
            root: t.path().to_path_buf(),
            db: None,
        },
        &DeadCodeParams {
            min_confidence: 0.0,
            scip: ScipOpts::disabled(),
            ..Default::default()
        },
    )
    .unwrap();
    let s: Vec<String> = r.findings.iter().filter_map(|f| f.symbol.clone()).collect();
    assert!(!s.contains(&"RealTools.RealMenu".to_string()), "{s:?}");
    assert!(
        !s.contains(&"QualifiedTools.QualifiedMenu".to_string()),
        "{s:?}"
    );
    assert!(s.contains(&"FakeTools.FakeMenu".to_string()), "{s:?}");
}

#[test]
fn unity_golden_default() {
    let t = copy_fixture("unity");
    let dbdir = tempfile::tempdir().unwrap();
    let r = run_dead_code(
        &Target {
            root: t.path().to_path_buf(),
            db: Some(dbdir.path().join("i.db")),
        },
        &DeadCodeParams {
            scip: ScipOpts::disabled(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_golden("unity_default", &golden_lines(&r.findings));
}

#[test]
fn unity_golden_include_low() {
    assert_golden("unity_low", &golden_lines(&all_findings()));
}
