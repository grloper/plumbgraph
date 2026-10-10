//! `plumb init` prints a Codex `config.toml` block. It must be valid TOML for every project
//! path, including Windows verbatim paths (`\\?\C:\...`) that `canonicalize` returns.
use plumbgraph_core::init::{codex_snippet, init};

/// Parse the snippet as TOML and return `mcp_servers.plumbgraph.args`.
fn args_of(snippet: &str) -> Vec<String> {
    let doc: toml::Table = toml::from_str(snippet)
        .unwrap_or_else(|e| panic!("snippet is not valid TOML: {e}\n---\n{snippet}"));
    let server = &doc["mcp_servers"]["plumbgraph"];
    assert_eq!(server["command"].as_str(), Some("plumb"));
    server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn codex_snippet_is_valid_toml_for_a_windows_verbatim_path() {
    let args = args_of(&codex_snippet(r"\\?\C:\Users\ofekv\Desktop\repos\Wraith"));
    assert_eq!(
        args,
        ["mcp", "--root", r"C:\Users\ofekv\Desktop\repos\Wraith"]
    );
}

#[test]
fn codex_snippet_maps_verbatim_unc_to_a_plain_unc_path() {
    let args = args_of(&codex_snippet(r"\\?\UNC\fileserver\share\proj"));
    assert_eq!(args[2], r"\\fileserver\share\proj");
}

#[test]
fn codex_snippet_round_trips_awkward_paths() {
    for p in [
        "/home/dev/my project",
        r"C:\Users\O'Brien\code",
        r#"C:\Users\x\"quoted"\dir"#,
        r"D:\a\b\c",
        "/tmp/it's \"both\"",
        "/tmp/tab\there",
        "/tmp/new\nline",
        "/tmp/bell\u{7}\u{7f}",
        "/tmp/ünïcødé/日本",
        r"\\server\share\already-plain",
    ] {
        let args = args_of(&codex_snippet(p));
        assert_eq!(args[2], p, "round trip of {p:?}");
    }
}

#[test]
fn plain_windows_paths_use_a_readable_literal_string() {
    let s = codex_snippet(r"\\?\C:\Users\me\proj");
    assert!(
        s.contains(r#"args = ["mcp", "--root", 'C:\Users\me\proj']"#),
        "{s}"
    );
}

#[test]
fn strip_verbatim_only_touches_verbatim_drive_and_unc_paths() {
    use plumbgraph_core::init::strip_verbatim;
    assert_eq!(strip_verbatim(r"\\?\C:\x"), r"C:\x");
    assert_eq!(strip_verbatim(r"\\?\d:\"), r"d:\");
    assert_eq!(strip_verbatim(r"\\?\UNC\srv\share"), r"\\srv\share");
    // no plain spelling exists for a volume GUID path: keep it
    assert_eq!(
        strip_verbatim(r"\\?\Volume{0b1c}\dir"),
        r"\\?\Volume{0b1c}\dir"
    );
    assert_eq!(strip_verbatim(r"\\?\"), r"\\?\");
    assert_eq!(strip_verbatim(r"C:\x"), r"C:\x");
    assert_eq!(strip_verbatim("/home/u/p"), "/home/u/p");
    assert_eq!(
        strip_verbatim(r"\\.\C:\x"),
        r"\\.\C:\x",
        "device paths untouched"
    );
}

#[test]
fn init_prints_a_parseable_codex_snippet_without_a_verbatim_prefix() {
    let t = tempfile::tempdir().unwrap();
    let rep = init(t.path(), &["codex".to_string()], true).unwrap();
    let snippet = rep
        .snippets
        .iter()
        .find(|s| s.contains("mcp_servers.plumbgraph"))
        .expect("codex snippet");
    let root = &args_of(snippet)[2];
    assert!(!root.starts_with(r"\\?\"), "{root}");
    assert!(
        std::path::Path::new(root).is_dir(),
        "the printed root must be a usable path: {root}"
    );
    for w in &rep.written {
        assert!(!w.starts_with(r"\\?\"), "reported paths are plain: {w}");
    }
}
