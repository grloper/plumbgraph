//! Test-weakening detection on a git diff: deleted tests, newly added skip/ignore/only
//! markers, reduced assertion counts, trivial or loosened assertions.
//!
//! Pattern-based and line-oriented: it reports *evidence from the diff*, with a confidence
//! per rule, and cannot judge whether a deletion was legitimate.

use crate::{sanitize, Finding, Severity};
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct WeakeningReport {
    pub base: String,
    pub base_commit: String,
    pub files_changed: usize,
    pub findings: Vec<Finding>,
    pub notes: Vec<String>,
}

#[derive(Debug, Default, Clone)]
struct FileDiff {
    path: String,
    deleted: bool,
    removed: Vec<(u32, String, usize)>, // (old line, text, hunk id)
    added: Vec<(u32, String, usize)>,   // (new line, text, hunk id)
}

fn parse_diff(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = vec![];
    let mut cur: Option<FileDiff> = None;
    let (mut old_ln, mut new_ln, mut hunk) = (0u32, 0u32, 0usize);
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            // "a/x b/x": take the b/ path
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, p)| p.to_string())
                .unwrap_or_default();
            cur = Some(FileDiff {
                path,
                ..Default::default()
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if !in_hunk {
            if line.starts_with("deleted file mode") {
                f.deleted = true;
            } else if let Some(p) = line.strip_prefix("--- a/") {
                if f.path.is_empty() {
                    f.path = p.to_string();
                }
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = p.to_string();
            }
        }
        if let Some(h) = line.strip_prefix("@@ ") {
            in_hunk = true;
            hunk += 1;
            // @@ -a,b +c,d @@
            let mut parts = h.split_whitespace();
            let old = parts.next().unwrap_or("-0");
            let new = parts.next().unwrap_or("+0");
            old_ln = old
                .trim_start_matches('-')
                .split(',')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            new_ln = new
                .trim_start_matches('+')
                .split(',')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            continue;
        }
        if !in_hunk {
            continue;
        }
        if let Some(t) = line.strip_prefix('-') {
            f.removed.push((old_ln, t.to_string(), hunk));
            old_ln += 1;
        } else if let Some(t) = line.strip_prefix('+') {
            f.added.push((new_ln, t.to_string(), hunk));
            new_ln += 1;
        }
    }
    if let Some(f) = cur.take() {
        files.push(f);
    }
    files
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Py,
    Js,
    Rs,
}

fn lang_of(path: &str) -> Option<Lang> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "py" => Some(Lang::Py),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => Some(Lang::Js),
        "rs" => Some(Lang::Rs),
        _ => None,
    }
}

fn is_test_file(path: &str, lang: Lang) -> bool {
    let lower = path.to_ascii_lowercase();
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    let in_dir = lower
        .split('/')
        .any(|c| matches!(c, "tests" | "test" | "__tests__" | "spec" | "specs"));
    match lang {
        Lang::Py => {
            in_dir
                || base.starts_with("test_")
                || base.ends_with("_test.py")
                || base == "conftest.py"
        }
        Lang::Js => in_dir || base.contains(".test.") || base.contains(".spec."),
        Lang::Rs => {
            in_dir
                || base.ends_with("_test.rs")
                || base.ends_with("_tests.rs")
                || base == "tests.rs"
        }
    }
}

struct Rules {
    py_test_def: Regex,
    js_test_def: Regex,
    rs_fn: Regex,
    skip_py: Regex,
    skip_js: Regex,
    only_js: Regex,
    ignore_rs: Regex,
    assert_py: Regex,
    assert_js: Regex,
    assert_rs: Regex,
    trivial: Regex,
    loose: Regex,
    strict: Regex,
}

impl Rules {
    fn new() -> Self {
        let r = |s: &str| Regex::new(s).expect("valid regex");
        Rules {
            py_test_def: r(r"^\s*(?:async\s+)?def\s+(test\w*)\s*\("),
            js_test_def: r(r#"^\s*(?:it|test)(?:\.\w+)?\s*\(\s*(?:'([^']*)'|"([^"]*)"|`([^`]*)`)"#),
            rs_fn: r(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)"),
            skip_py: r(
                r"@(?:pytest\.mark\.(?:skip|skipif|xfail)|unittest\.(?:skip|skipIf|skipUnless|expectedFailure))\b|\bpytest\.(?:skip|xfail)\s*\(|\bself\.skipTest\s*\(",
            ),
            skip_js: r(
                r"\b(?:it|test|describe)\.(?:skip|todo|failing)\b|\b(?:xit|xtest|xdescribe)\s*\(",
            ),
            only_js: r(r"\b(?:it|test|describe)\.only\b|\b(?:fit|fdescribe)\s*\("),
            ignore_rs: r(r"^\s*#\[\s*ignore\b"),
            assert_py: r(r"^\s*assert\b|\bself\.assert\w+\s*\(|\bpytest\.raises\s*\("),
            assert_js: r(r"\bexpect\s*\(|\bassert(?:\.\w+)?\s*\(|\.should\b"),
            assert_rs: r(r"\b(?:debug_)?assert(?:_eq|_ne)?!"),
            trivial: r(
                r"^\s*assert\s+(?:True|1)\s*$|expect\s*\(\s*true\s*\)\s*\.toBe\s*\(\s*true\s*\)|expect\.anything\s*\(\s*\)|\bassert!\s*\(\s*true\s*\)|\bassertTrue\s*\(\s*True\s*\)",
            ),
            loose: r(
                r"toBeTruthy|toBeDefined|not\.toBeNull|not\.toBeUndefined|\bis not None\b|assertIsNotNone|assertTrue\s*\(|\.is_some\(\)|\.is_ok\(\)",
            ),
            strict: r(r"toBe\(|toEqual\(|toStrictEqual\(|==|assertEqual|assertEquals|assert_eq!"),
        }
    }
}

fn test_name(rules: &Rules, lang: Lang, text: &str) -> Option<String> {
    match lang {
        Lang::Py => rules.py_test_def.captures(text).map(|c| c[1].to_string()),
        Lang::Js => rules.js_test_def.captures(text).and_then(|c| {
            c.get(1)
                .or(c.get(2))
                .or(c.get(3))
                .map(|m| m.as_str().to_string())
        }),
        Lang::Rs => None,
    }
}

fn snippet(s: &str) -> String {
    sanitize(s.trim())
}

/// Analyse unified diff text (`git diff -U0`).
pub fn analyze_diff(diff: &str) -> Vec<Finding> {
    let rules = Rules::new();
    let files = parse_diff(diff);
    // names of tests that appear as added anywhere in the diff (moved/renamed tests)
    let mut added_names: HashSet<String> = HashSet::new();
    for f in &files {
        let Some(lang) = lang_of(&f.path) else {
            continue;
        };
        for (i, (_, t, h)) in f.added.iter().enumerate() {
            if let Some(n) = test_name(&rules, lang, t) {
                added_names.insert(n);
            }
            if lang == Lang::Rs
                && f.added[..i]
                    .iter()
                    .rev()
                    .take(3)
                    .any(|(_, p, hh)| hh == h && p.trim() == "#[test]")
            {
                if let Some(c) = rules.rs_fn.captures(t) {
                    added_names.insert(c[1].to_string());
                }
            }
        }
    }
    let mut out: Vec<Finding> = vec![];
    for f in &files {
        let Some(lang) = lang_of(&f.path) else {
            continue;
        };
        let test_file = is_test_file(&f.path, lang);
        let mut deleted_tests_in_file = 0usize;
        if f.deleted && test_file {
            let mut fd = Finding::new(
                "weakening",
                "deleted-test-file",
                &f.path,
                1,
                0.85,
                "git-diff",
                format!("test file `{}` was deleted", sanitize(&f.path)),
            );
            fd.severity = Severity::Error;
            fd.kind = "test-file".into();
            fd.evidence = vec![format!("{} line(s) removed with the file", f.removed.len())];
            fd.fp_risks = vec!["the tests may have been moved to a file outside this diff's path scope, or were obsolete".into()];
            out.push(fd);
            continue;
        }
        // deleted tests
        for (i, (ln, t, h)) in f.removed.iter().enumerate() {
            let name = match lang {
                Lang::Rs => {
                    // `#[test]` removed, followed by the fn line in the same hunk
                    if t.trim_start().starts_with("#[test]")
                        || t.trim_start().starts_with("#[tokio::test")
                    {
                        f.removed[i + 1..]
                            .iter()
                            .take(4)
                            .filter(|(_, _, hh)| hh == h)
                            .find_map(|(_, x, _)| rules.rs_fn.captures(x).map(|c| c[1].to_string()))
                    } else {
                        None
                    }
                }
                _ => {
                    if test_file || lang == Lang::Js {
                        test_name(&rules, lang, t)
                    } else {
                        None
                    }
                }
            };
            let Some(name) = name else { continue };
            if added_names.contains(&name) {
                continue;
            }
            deleted_tests_in_file += 1;
            let mut fd = Finding::new(
                "weakening",
                "deleted-test",
                &f.path,
                *ln,
                0.80,
                "git-diff",
                format!("test `{}` was removed", sanitize(&name)),
            );
            fd.severity = Severity::Error;
            fd.kind = "test".into();
            fd.symbol = Some(sanitize(&name));
            fd.evidence = vec![
                format!("removed line: `{}`", snippet(t)),
                "no test with the same name is added anywhere in this diff".into(),
            ];
            fd.fp_risks = vec!["the test may have been intentionally replaced by differently-named tests, or the tested code was removed".into()];
            out.push(fd);
        }
        // added skip / ignore / focus markers
        for (ln, t, _) in &f.added {
            let hit = match lang {
                Lang::Py if test_file => {
                    rules
                        .skip_py
                        .is_match(t)
                        .then_some(("added-skip", 0.95, "skip/xfail marker"))
                }
                Lang::Js if test_file => rules
                    .skip_js
                    .is_match(t)
                    .then_some(("added-skip", 0.95, "skip/todo marker"))
                    .or_else(|| {
                        rules.only_js.is_match(t).then_some((
                            "added-focus",
                            0.90,
                            ".only/focus marker (silently disables all other tests in the run)",
                        ))
                    }),
                Lang::Rs => {
                    rules
                        .ignore_rs
                        .is_match(t)
                        .then_some(("added-skip", 0.95, "#[ignore]"))
                }
                _ => None,
            };
            if let Some((rule, conf, what)) = hit {
                let mut fd = Finding::new(
                    "weakening",
                    rule,
                    &f.path,
                    *ln,
                    conf,
                    "git-diff",
                    format!("{what} added"),
                );
                fd.severity = Severity::Error;
                fd.kind = "marker".into();
                fd.evidence = vec![format!("added line: `{}`", snippet(t))];
                fd.fp_risks =
                    vec!["conditional skips (e.g. platform-specific) can be legitimate".into()];
                out.push(fd);
            }
            if test_file && rules.trivial.is_match(t) {
                let mut fd = Finding::new(
                    "weakening",
                    "trivial-assertion",
                    &f.path,
                    *ln,
                    0.85,
                    "git-diff",
                    "assertion that cannot fail".into(),
                );
                fd.severity = Severity::Warning;
                fd.kind = "assertion".into();
                fd.evidence = vec![format!("added line: `{}`", snippet(t))];
                fd.fp_risks = vec!["placeholders in new tests are common; check intent".into()];
                out.push(fd);
            }
        }
        // assertion counts (test files only)
        if test_file {
            let re = match lang {
                Lang::Py => &rules.assert_py,
                Lang::Js => &rules.assert_js,
                Lang::Rs => &rules.assert_rs,
            };
            let removed_n = f.removed.iter().filter(|(_, t, _)| re.is_match(t)).count();
            let added_n = f.added.iter().filter(|(_, t, _)| re.is_match(t)).count();
            if removed_n > added_n {
                let drop = removed_n - added_n;
                let first = f
                    .removed
                    .iter()
                    .find(|(_, t, _)| re.is_match(t))
                    .map(|(l, _, _)| *l)
                    .unwrap_or(1);
                let explained = deleted_tests_in_file > 0;
                let conf = if explained {
                    0.40
                } else if drop >= 3 {
                    0.65
                } else {
                    0.55
                };
                let mut fd = Finding::new("weakening", "assertions-reduced", &f.path, first, conf, "git-diff", format!("net {drop} fewer assertion(s) in this file ({removed_n} removed, {added_n} added)"));
                fd.severity = Severity::Warning;
                fd.kind = "assertion".into();
                fd.evidence = vec![format!(
                    "{removed_n} assertion line(s) removed, {added_n} added"
                )];
                fd.fp_risks = vec![
                    "assertions may have moved to helpers or been consolidated".into(),
                    if explained {
                        "some of the removed assertions belong to tests that were deleted (see deleted-test findings)".into()
                    } else {
                        "line-based count; refactors can change it without weakening".into()
                    },
                ];
                out.push(fd);
            }
            // loosened: within one hunk, strict assertion removed, loose assertion added without strict
            for (ln, t, h) in &f.added {
                if rules.loose.is_match(t)
                    && !rules.strict.is_match(t)
                    && f.removed
                        .iter()
                        .any(|(_, r, rh)| rh == h && re.is_match(r) && rules.strict.is_match(r))
                {
                    let mut fd = Finding::new(
                        "weakening",
                        "assertion-loosened",
                        &f.path,
                        *ln,
                        0.60,
                        "git-diff",
                        "an equality assertion in this hunk was replaced by a weaker check".into(),
                    );
                    fd.severity = Severity::Warning;
                    fd.kind = "assertion".into();
                    fd.evidence = vec![format!("added line: `{}`", snippet(t))];
                    fd.fp_risks = vec!["pairing is by hunk, not by statement".into()];
                    out.push(fd);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false", "-c", "core.pager=cat"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("running git (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Diff `base` against the working tree (or against `head` when given) and analyse it.
pub fn detect(repo: &Path, base: &str, head: Option<&str>) -> Result<WeakeningReport> {
    for r in std::iter::once(base).chain(head) {
        if r.is_empty() || r.starts_with('-') || r.contains('\0') || r.contains('\n') {
            bail!("invalid revision `{}`", sanitize(r));
        }
    }
    let base_commit = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )
    .map_err(|_| anyhow::anyhow!("revision `{}` not found in this repository", sanitize(base)))?
    .trim()
    .to_string();
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--unified=0",
        "--find-renames",
        &base_commit,
    ];
    let head_commit;
    if let Some(h) = head {
        head_commit = git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{h}^{{commit}}"),
            ],
        )
        .map_err(|_| anyhow::anyhow!("revision `{}` not found in this repository", sanitize(h)))?
        .trim()
        .to_string();
        args.push(&head_commit);
    }
    args.push("--");
    let diff = git(repo, &args)?;
    let files_changed = diff
        .lines()
        .filter(|l| l.starts_with("diff --git "))
        .count();
    let mut notes = vec![
        "untracked (never `git add`ed) files are not part of `git diff` and are not analysed"
            .to_string(),
    ];
    if head.is_none() {
        notes.push(
            "compared against the working tree (tracked files, staged and unstaged changes)".into(),
        );
    }
    Ok(WeakeningReport {
        base: base.into(),
        base_commit,
        files_changed,
        findings: analyze_diff(&diff),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(findings: &[Finding]) -> Vec<&str> {
        let mut v: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
        v.sort();
        v
    }

    #[test]
    fn python_deleted_test_and_skip() {
        let d = "diff --git a/tests/test_a.py b/tests/test_a.py\n--- a/tests/test_a.py\n+++ b/tests/test_a.py\n@@ -3,4 +2,0 @@\n-def test_removed():\n-    assert f() == 1\n-    assert g() == 2\n-    assert h() == 3\n@@ -10,0 +7,2 @@\n+@pytest.mark.skip(reason='flaky')\n+def test_kept():\n";
        let f = analyze_diff(d);
        let r = rules(&f);
        assert!(
            r.contains(&"deleted-test")
                && r.contains(&"added-skip")
                && r.contains(&"assertions-reduced"),
            "{r:?}"
        );
        let del = f.iter().find(|x| x.rule == "deleted-test").unwrap();
        assert_eq!(del.symbol.as_deref(), Some("test_removed"));
        assert_eq!(del.line, 3);
        let skip = f.iter().find(|x| x.rule == "added-skip").unwrap();
        assert_eq!(skip.line, 7);
        assert!(skip.confidence >= 0.9 && skip.source == "git-diff");
        // assertion drop is explained by the deleted test -> lowered confidence
        assert!(
            f.iter()
                .find(|x| x.rule == "assertions-reduced")
                .unwrap()
                .confidence
                < 0.5
        );
    }

    #[test]
    fn moved_test_is_not_a_deletion() {
        let d = "diff --git a/tests/test_a.py b/tests/test_a.py\n--- a/tests/test_a.py\n+++ b/tests/test_a.py\n@@ -1,1 +0,0 @@\n-def test_moved():\n@@ -0,0 +1,1 @@\n+def test_moved():\ndiff --git a/tests/test_b.py b/tests/test_b.py\n--- a/tests/test_b.py\n+++ b/tests/test_b.py\n@@ -0,0 +1,1 @@\n+def test_moved():\n";
        assert!(!analyze_diff(d).iter().any(|f| f.rule == "deleted-test"));
    }

    #[test]
    fn js_markers() {
        let d = "diff --git a/src/a.test.ts b/src/a.test.ts\n--- a/src/a.test.ts\n+++ b/src/a.test.ts\n@@ -5,1 +5,1 @@\n-  it('works', () => {\n+  it.skip('works', () => {\n@@ -9,0 +10,2 @@\n+  test.only('focus', () => {});\n+  expect(true).toBe(true);\n";
        let f = analyze_diff(d);
        let r = rules(&f);
        assert!(
            r.contains(&"added-skip")
                && r.contains(&"added-focus")
                && r.contains(&"trivial-assertion"),
            "{r:?}"
        );
        // `it('works')` was removed and `it.skip('works')` added with the same name: not a deletion
        assert!(!r.contains(&"deleted-test"));
    }

    #[test]
    fn rust_ignore_only_counts_as_attribute_line() {
        let hdr = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,0 +2,1 @@\n";
        // the text appears inside a string literal / call: not a skip marker
        let d = format!("{hdr}+    let s = \"#[ignore]\";\n");
        assert!(!rules(&analyze_diff(&d)).contains(&"added-skip"));
        // a real attribute line, with a reason, is one
        let d = format!("{hdr}+    #[ignore = \"slow\"]\n");
        assert!(rules(&analyze_diff(&d)).contains(&"added-skip"));
    }

    #[test]
    fn rust_ignore_and_removed_test() {
        let d = "diff --git a/tests/it.rs b/tests/it.rs\n--- a/tests/it.rs\n+++ b/tests/it.rs\n@@ -4,3 +3,0 @@\n-#[test]\n-fn gone() {\n-    assert_eq!(1, 1);\n@@ -20,0 +17,1 @@\n+#[ignore]\n";
        let f = analyze_diff(d);
        let r = rules(&f);
        assert!(
            r.contains(&"deleted-test") && r.contains(&"added-skip"),
            "{r:?}"
        );
        assert_eq!(
            f.iter()
                .find(|x| x.rule == "deleted-test")
                .unwrap()
                .symbol
                .as_deref(),
            Some("gone")
        );
    }

    #[test]
    fn deleted_test_file_and_non_test_files_ignored() {
        let d = "diff --git a/tests/test_x.py b/tests/test_x.py\ndeleted file mode 100644\n--- a/tests/test_x.py\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-def test_x():\n-    assert 1\ndiff --git a/src/app.py b/src/app.py\n--- a/src/app.py\n+++ b/src/app.py\n@@ -1,1 +0,0 @@\n-def test_helper_in_prod():\n";
        let f = analyze_diff(d);
        assert_eq!(rules(&f), vec!["deleted-test-file"]);
    }

    #[test]
    fn loosened_assertion() {
        let d = "diff --git a/tests/test_a.py b/tests/test_a.py\n--- a/tests/test_a.py\n+++ b/tests/test_a.py\n@@ -3,1 +3,1 @@\n-    self.assertEqual(f(), 3)\n+    self.assertTrue(f())\n";
        assert!(analyze_diff(d)
            .iter()
            .any(|f| f.rule == "assertion-loosened"));
    }

    fn sh(dir: &Path, args: &[&str]) {
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
        assert!(s.success(), "git {args:?}");
    }

    #[test]
    fn end_to_end_with_real_git_repo() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        sh(d, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(d.join("tests")).unwrap();
        std::fs::write(
            d.join("tests/test_a.py"),
            "def test_one():\n    assert 1 == 1\n\ndef test_two():\n    assert 2 == 2\n",
        )
        .unwrap();
        sh(d, &["add", "."]);
        sh(d, &["commit", "-q", "-m", "init"]);
        std::fs::write(
            d.join("tests/test_a.py"),
            "def test_one():\n    assert 1 == 1\n",
        )
        .unwrap();
        let rep = detect(d, "HEAD", None).unwrap();
        assert_eq!(rep.files_changed, 1);
        assert!(rep
            .findings
            .iter()
            .any(|f| f.rule == "deleted-test" && f.symbol.as_deref() == Some("test_two")));
        assert!(detect(d, "--output=/tmp/x", None).is_err());
        assert!(detect(d, "no-such-rev", None).is_err());
    }
}
