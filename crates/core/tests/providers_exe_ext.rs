//! Provider detection must find Windows executables (`cargo.exe`, npm `.cmd` shims) without
//! changing the exact-name lookup used on Unix. The extension list is injected so the Windows
//! rules run on every platform; one `cfg(windows)` test covers the real `PATHEXT` path.
use plumbgraph_core::providers::{self, Detected};
use std::path::{Path, PathBuf};

/// Create an executable file: mode 0755 on Unix, plain file elsewhere.
fn exe(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn project() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("main.rs"), "fn main() {}\n").unwrap();
    t
}

fn get<'a>(det: &'a [Detected], id: &str) -> &'a Detected {
    det.iter().find(|d| d.id == id).unwrap()
}

fn file_name(d: &Detected) -> String {
    Path::new(d.path.as_deref().unwrap_or_default())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn win_exts() -> Vec<String> {
    [".com", ".exe", ".bat", ".cmd"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn windows_extensions_find_exe_and_cmd_programs() {
    let bin = tempfile::tempdir().unwrap();
    exe(bin.path(), "cargo.exe");
    exe(bin.path(), "cargo-clippy.exe");
    // npm installs an extensionless sh shim next to the .cmd one; only the .cmd runs on Windows
    exe(bin.path(), "tsc");
    exe(bin.path(), "tsc.cmd");
    exe(bin.path(), "semgrep.bat");
    let t = project();
    exe(t.path(), "node_modules/.bin/ast-grep.cmd");
    let det = providers::detect_with_exts(t.path(), &[bin.path().to_path_buf()], &win_exts());

    let cargo = get(&det, "cargo-check");
    assert!(cargo.available, "{cargo:?}");
    assert_eq!(file_name(cargo), "cargo.exe");
    assert!(get(&det, "clippy").available);
    assert_eq!(
        file_name(get(&det, "tsc")),
        "tsc.cmd",
        "the sh shim cannot run on Windows"
    );
    assert_eq!(file_name(get(&det, "semgrep")), "semgrep.bat");
    assert_eq!(
        file_name(get(&det, "ast-grep")),
        "ast-grep.cmd",
        "project-local node_modules/.bin"
    );
    assert!(!get(&det, "ruff").available, "nothing named ruff exists");
}

#[test]
fn windows_extension_order_follows_the_list() {
    // PATHEXT order decides between two candidates in the same directory, as cmd.exe does
    let bin = tempfile::tempdir().unwrap();
    exe(bin.path(), "ruff.cmd");
    exe(bin.path(), "ruff.exe");
    let t = project();
    let det = providers::detect_with_exts(t.path(), &[bin.path().to_path_buf()], &win_exts());
    assert_eq!(file_name(get(&det, "ruff")), "ruff.exe");
    // and the first directory on the search path wins over a better extension later on
    let first = tempfile::tempdir().unwrap();
    exe(first.path(), "ruff.bat");
    let det = providers::detect_with_exts(
        t.path(),
        &[first.path().to_path_buf(), bin.path().to_path_buf()],
        &win_exts(),
    );
    assert_eq!(file_name(get(&det, "ruff")), "ruff.bat");
}

#[test]
fn no_extension_list_keeps_exact_name_lookup() {
    // the Unix behaviour: `cargo.exe` is not `cargo`
    let bin = tempfile::tempdir().unwrap();
    exe(bin.path(), "cargo.exe");
    exe(bin.path(), "ruff");
    let t = project();
    let det = providers::detect_with_exts(t.path(), &[bin.path().to_path_buf()], &[]);
    assert!(!get(&det, "cargo-check").available);
    let ruff = get(&det, "ruff");
    assert!(ruff.available, "{ruff:?}");
    assert_eq!(file_name(ruff), "ruff");
}

#[test]
fn pathext_parsing_normalises_and_keeps_only_runnable_extensions() {
    assert_eq!(
        providers::parse_pathext(".COM;.EXE;;.Bat ; cmd;.EXE;.VBS;.JS;.PS1;.MSC"),
        vec![".com", ".exe", ".bat", ".cmd"],
        "case folded, blanks and duplicates dropped, dot added, non-runnable hosts dropped"
    );
    assert_eq!(
        providers::parse_pathext(".CMD;.EXE"),
        vec![".cmd", ".exe"],
        "user order is kept"
    );
    assert!(providers::parse_pathext("").is_empty());
    assert!(providers::parse_pathext(".JS;.VBS").is_empty());
    assert_eq!(
        providers::parse_pathext(providers::DEFAULT_PATHEXT),
        vec![".com", ".exe", ".bat", ".cmd"]
    );
}

#[test]
fn a_non_runnable_extension_is_not_reported_as_installed() {
    // `ruff.js` would need a script host; Command cannot start it
    let bin = tempfile::tempdir().unwrap();
    exe(bin.path(), "ruff.js");
    let t = project();
    let exts = providers::parse_pathext(".COM;.EXE;.BAT;.CMD;.JS");
    let det = providers::detect_with_exts(t.path(), &[bin.path().to_path_buf()], &exts);
    assert!(!get(&det, "ruff").available);
}

#[test]
fn platform_extension_list_matches_the_platform() {
    let exts = providers::executable_extensions();
    if cfg!(windows) {
        assert!(exts.iter().any(|e| e == ".exe"), "{exts:?}");
    } else {
        assert!(
            exts.is_empty(),
            "Unix lookup must stay exact-name: {exts:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn unix_default_detection_is_exact_name_only() {
    let bin = tempfile::tempdir().unwrap();
    exe(bin.path(), "cargo.exe");
    let t = project();
    let det = providers::detect(t.path(), &[bin.path().to_path_buf()]);
    assert!(!get(&det, "cargo-check").available);
}

#[cfg(windows)]
#[test]
fn real_windows_detection_finds_cargo_exe() {
    let bin = tempfile::tempdir().unwrap();
    exe(bin.path(), "cargo.exe");
    let t = project();
    let det = providers::detect(t.path(), &[bin.path().to_path_buf()]);
    let cargo = get(&det, "cargo-check");
    assert!(cargo.available, "{cargo:?}");
    assert!(file_name(cargo).eq_ignore_ascii_case("cargo.exe"));
}
