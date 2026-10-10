//! `plumb init` keeps the local index (`.plumbgraph/index.db`, SCIP files) out of git while
//! the allow-list `.plumbgraph/allow.toml` stays committable, and says so in `--dry-run`.
use plumbgraph_core::init::init;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}

fn repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    assert!(git(t.path(), &["init", "-q"]).status.success());
    t
}

/// Is `rel` ignored by git in `dir`? (`git check-ignore` exits 0 when it is.)
fn ignored(dir: &Path, rel: &str) -> bool {
    git(dir, &["check-ignore", "-q", "--no-index", rel])
        .status
        .success()
}

fn gitignore(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default()
}

#[test]
fn index_is_ignored_but_the_allow_list_stays_committable() {
    let t = repo();
    init(t.path(), &[], false).unwrap();
    assert!(
        ignored(t.path(), ".plumbgraph/index.db"),
        "{}",
        gitignore(t.path())
    );
    assert!(ignored(t.path(), ".plumbgraph/scip/scip-python.scip"));
    assert!(
        !ignored(t.path(), ".plumbgraph/allow.toml"),
        "allow.toml is documented as committed: {}",
        gitignore(t.path())
    );
}

#[test]
fn dry_run_reports_the_gitignore_lines_and_writes_nothing() {
    let t = repo();
    let rep = init(t.path(), &[], true).unwrap();
    assert!(!t.path().join(".gitignore").exists());
    let said = format!("{:?}", rep.notes);
    assert!(said.contains(".plumbgraph/*"), "{said}");
    assert!(said.contains("!.plumbgraph/allow.toml"), "{said}");
}

#[test]
fn equivalent_existing_patterns_are_respected() {
    for existing in [
        ".plumbgraph/\n",
        "/.plumbgraph\n",
        "**/.plumbgraph/*\n!**/.plumbgraph/allow.toml\n",
        ".plumbgraph/*\r\n",
        // the user deliberately commits the index: appending `.plumbgraph/*` would win
        // (last match) and silently undo that
        "!.plumbgraph/\n",
        ".plumbgraph*\n",
    ] {
        let t = repo();
        std::fs::write(t.path().join(".gitignore"), existing).unwrap();
        let rep = init(t.path(), &[], false).unwrap();
        assert_eq!(gitignore(t.path()), existing, "left untouched");
        assert!(
            !rep.written.iter().any(|w| w.ends_with(".gitignore")),
            "{rep:?}"
        );
    }
}

#[test]
fn a_project_in_a_subdirectory_of_a_repo_gets_its_own_entry() {
    let t = repo();
    let sub = t.path().join("services/api");
    std::fs::create_dir_all(&sub).unwrap();
    init(&sub, &[], false).unwrap();
    assert!(ignored(t.path(), "services/api/.plumbgraph/index.db"));
    assert!(!ignored(t.path(), "services/api/.plumbgraph/allow.toml"));
    assert!(
        !t.path().join(".gitignore").exists(),
        "the repo root is not touched"
    );
}

#[test]
fn crlf_gitignore_keeps_its_line_endings() {
    let t = repo();
    std::fs::write(t.path().join(".gitignore"), "target/\r\n").unwrap();
    init(t.path(), &[], false).unwrap();
    let g = gitignore(t.path());
    assert!(g.starts_with("target/\r\n"), "{g:?}");
    assert!(
        !g.replace("\r\n", "").contains('\n'),
        "no bare LF mixed in: {g:?}"
    );
    assert!(ignored(t.path(), ".plumbgraph/index.db"));
}

#[test]
fn a_gitignore_that_is_not_utf8_is_never_overwritten() {
    // what Windows PowerShell 5 writes for `echo target/ > .gitignore`: UTF-16LE with a BOM
    let t = repo();
    let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
    for u in "target/\r\n".encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(t.path().join(".gitignore"), &utf16).unwrap();
    let rep = init(t.path(), &[], false).unwrap();
    assert_eq!(
        std::fs::read(t.path().join(".gitignore")).unwrap(),
        utf16,
        "user content must survive"
    );
    assert!(
        rep.notes.iter().any(|n| n.contains("not UTF-8")),
        "{:?}",
        rep.notes
    );
}

#[test]
fn bom_prefixed_existing_entry_is_recognised() {
    let t = repo();
    let existing = "\u{feff}.plumbgraph/\n";
    std::fs::write(t.path().join(".gitignore"), existing).unwrap();
    init(t.path(), &[], false).unwrap();
    assert_eq!(gitignore(t.path()), existing);
}

#[test]
fn no_gitignore_option_leaves_it_alone_and_says_what_to_add() {
    let t = repo();
    let rep = plumbgraph_core::init::init_with(t.path(), &[], false, false).unwrap();
    assert!(!t.path().join(".gitignore").exists());
    let said = format!("{:?}", rep.notes);
    assert!(
        said.contains("--no-gitignore") && said.contains(".plumbgraph/*"),
        "{said}"
    );
    // the agent files are still written
    assert!(t.path().join("AGENTS.md").is_file());
}

#[test]
fn second_run_is_a_no_op() {
    let t = repo();
    init(t.path(), &[], false).unwrap();
    let first = gitignore(t.path());
    let again = init(t.path(), &[], false).unwrap();
    assert_eq!(gitignore(t.path()), first);
    assert!(again.written.is_empty(), "{again:?}");
}

#[test]
fn outside_git_nothing_is_written_and_the_report_says_why() {
    let t = tempfile::tempdir().unwrap();
    let rep = init(t.path(), &[], false).unwrap();
    assert!(!t.path().join(".gitignore").exists());
    assert!(
        rep.notes.iter().any(|n| n.contains("not inside a git")),
        "{:?}",
        rep.notes
    );
}
