#![allow(dead_code)]
use std::path::{Path, PathBuf};

pub fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            copy_dir(&p, &dst.join(&name));
        } else {
            // manifests are stored as `*.fixture` so GitHub/dependabot do not scan them
            let out = name.strip_suffix(".fixture").unwrap_or(&name).to_string();
            std::fs::copy(&p, dst.join(out)).unwrap();
        }
    }
}

/// Copy a fixture into a temp dir (so tests never write into the repository).
pub fn copy_fixture(name: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir(&fixture_dir(name), t.path());
    t
}

/// Render findings as stable golden lines.
pub fn golden_lines(findings: &[plumbgraph_core::Finding]) -> String {
    let mut lines: Vec<String> = findings
        .iter()
        .map(|f| {
            format!(
                "{:<6} {:.2} {:<18} {:<9} {} {}:{}",
                f.level.as_str(),
                f.confidence,
                f.rule,
                f.kind,
                f.symbol.clone().unwrap_or_default(),
                f.file,
                f.line
            )
        })
        .collect();
    lines.sort();
    lines.join("\n") + "\n"
}

/// Compare against `tests/golden/<name>.txt`; set PLUMB_BLESS=1 to (re)write it.
pub fn assert_golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"));
    if std::env::var_os("PLUMB_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden file {}; run with PLUMB_BLESS=1",
            path.display()
        )
    });
    assert_eq!(actual, expected, "golden mismatch for {name}");
}
