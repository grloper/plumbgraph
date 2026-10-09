//! Manifest parsing: Cargo.toml, package.json, pyproject.toml, requirements*.txt.
//! Only declarations are read; nothing is executed or installed.

use serde_json::Value as Json;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDep {
    /// Name as used when importing / as the manifest key.
    pub name: String,
    /// Name to look up in the registry (differs for renames/aliases).
    pub registry_name: String,
    pub section: String,
    /// False for path/git/url/workspace dependencies (never looked up on a registry).
    pub checkable: bool,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    /// project-relative path using `/`
    pub path: String,
    /// project-relative directory ("" = root)
    pub dir: String,
    /// `pypi` | `npm` | `crates.io`
    pub eco: &'static str,
    pub package_name: Option<String>,
    pub deps: Vec<DeclaredDep>,
}

pub fn is_manifest_name(name: &str) -> Option<&'static str> {
    match name {
        "Cargo.toml" => Some("crates.io"),
        "package.json" => Some("npm"),
        "pyproject.toml" => Some("pypi"),
        n if n.starts_with("requirements") && n.ends_with(".txt") => Some("pypi"),
        _ => None,
    }
}

fn line_of(text: &str, needle: &str) -> u32 {
    text.lines()
        .position(|l| l.contains(needle))
        .map(|i| i as u32 + 1)
        .unwrap_or(0)
}

pub fn parse(rel_path: &str, text: &str) -> Option<Manifest> {
    let name = rel_path.rsplit('/').next()?;
    let eco = is_manifest_name(name)?;
    let dir = rel_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    let (package_name, deps) = match name {
        "Cargo.toml" => parse_cargo(text)?,
        "package.json" => parse_package_json(text)?,
        "pyproject.toml" => parse_pyproject(text)?,
        _ => (None, parse_requirements(text)),
    };
    Some(Manifest {
        path: rel_path.into(),
        dir,
        eco,
        package_name,
        deps,
    })
}

pub fn read(root: &Path, rel_path: &str) -> Option<Manifest> {
    let text = std::fs::read_to_string(root.join(rel_path)).ok()?;
    parse(rel_path, &text)
}

fn parse_cargo(text: &str) -> Option<(Option<String>, Vec<DeclaredDep>)> {
    let v: toml::Value = toml::from_str(text).ok()?;
    let pkg = v
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from);
    let mut deps = vec![];
    let mut sections: Vec<(String, &toml::Value)> = vec![];
    for s in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(t) = v.get(s) {
            sections.push((s.into(), t));
        }
    }
    if let Some(t) = v.get("workspace").and_then(|w| w.get("dependencies")) {
        sections.push(("workspace.dependencies".into(), t));
    }
    if let Some(targets) = v.get("target").and_then(|t| t.as_table()) {
        for (tn, tv) in targets {
            for s in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(t) = tv.get(s) {
                    sections.push((format!("target.{tn}.{s}"), t));
                }
            }
        }
    }
    for (sec, t) in sections {
        let Some(tbl) = t.as_table() else { continue };
        for (k, val) in tbl {
            let mut registry_name = k.clone();
            let mut checkable = true;
            if let Some(t) = val.as_table() {
                if t.contains_key("path") || t.contains_key("git") {
                    checkable = false;
                }
                if t.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                    checkable = false; // checked at the workspace.dependencies entry
                }
                if let Some(p) = t.get("package").and_then(|p| p.as_str()) {
                    registry_name = p.to_string();
                }
                if t.contains_key("registry") {
                    checkable = false; // alternative registry
                }
            }
            deps.push(DeclaredDep {
                name: k.clone(),
                registry_name,
                section: sec.clone(),
                checkable,
                line: line_of(text, k),
            });
        }
    }
    Some((pkg, deps))
}

fn parse_package_json(text: &str) -> Option<(Option<String>, Vec<DeclaredDep>)> {
    let v: Json = serde_json::from_str(text).ok()?;
    let name = v.get("name").and_then(|n| n.as_str()).map(String::from);
    let mut deps = vec![];
    for sec in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(o) = v.get(sec).and_then(|d| d.as_object()) else {
            continue;
        };
        for (k, val) in o {
            let spec = val.as_str().unwrap_or("");
            let mut registry_name = k.clone();
            let mut checkable = true;
            if let Some(alias) = spec.strip_prefix("npm:") {
                // npm:real-name@^1
                let real = if let Some(rest) = alias.strip_prefix('@') {
                    format!("@{}", rest.split('@').next().unwrap_or(rest))
                } else {
                    alias.split('@').next().unwrap_or(alias).to_string()
                };
                registry_name = real;
            } else if spec.starts_with("file:")
                || spec.starts_with("link:")
                || spec.starts_with("git")
                || spec.starts_with("github:")
                || spec.starts_with("http")
                || spec.starts_with("workspace:")
                || spec.starts_with("portal:")
                || spec.contains('/')
                    && !spec.starts_with('@')
                    && !spec.contains(' ')
                    && !spec.starts_with('^')
                    && !spec.starts_with('~')
                    && !spec.starts_with('>')
                    && !spec.starts_with('<')
                    && !spec.starts_with('=')
            {
                checkable = false;
            }
            deps.push(DeclaredDep {
                name: k.clone(),
                registry_name,
                section: sec.into(),
                checkable,
                line: line_of(text, &format!("\"{k}\"")),
            });
        }
    }
    Some((name, deps))
}

/// Parse a PEP 508 requirement: returns (name, is_direct_url).
fn parse_req(s: &str) -> Option<(String, bool)> {
    let s = s.trim();
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .unwrap_or(s.len());
    let name = &s[..end];
    if name.is_empty() || !name.chars().next()?.is_ascii_alphanumeric() {
        return None;
    }
    let rest = s[end..].trim_start();
    let direct =
        rest.starts_with('@') || rest.starts_with("[") && rest.contains(" @ ") || s.contains("://");
    Some((name.to_string(), direct))
}

fn parse_pyproject(text: &str) -> Option<(Option<String>, Vec<DeclaredDep>)> {
    let v: toml::Value = toml::from_str(text).ok()?;
    let name = v
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from)
        .or_else(|| {
            v.get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .map(String::from)
        });
    let mut deps = vec![];
    let mut add_list = |sec: &str, arr: &toml::Value| {
        if let Some(a) = arr.as_array() {
            for item in a {
                if let Some(s) = item.as_str() {
                    if let Some((n, direct)) = parse_req(s) {
                        deps.push(DeclaredDep {
                            name: n.clone(),
                            registry_name: n,
                            section: sec.into(),
                            checkable: !direct,
                            line: line_of(text, s),
                        });
                    }
                }
            }
        }
    };
    if let Some(p) = v.get("project") {
        if let Some(d) = p.get("dependencies") {
            add_list("project.dependencies", d);
        }
        if let Some(o) = p.get("optional-dependencies").and_then(|o| o.as_table()) {
            for (k, d) in o {
                add_list(&format!("project.optional-dependencies.{k}"), d);
            }
        }
    }
    if let Some(o) = v.get("dependency-groups").and_then(|o| o.as_table()) {
        for (k, d) in o {
            add_list(&format!("dependency-groups.{k}"), d);
        }
    }
    if let Some(b) = v.get("build-system").and_then(|b| b.get("requires")) {
        add_list("build-system.requires", b);
    }
    let mut poetry_tables: Vec<(String, &toml::Value)> = vec![];
    if let Some(poetry) = v.get("tool").and_then(|t| t.get("poetry")) {
        for s in ["dependencies", "dev-dependencies"] {
            if let Some(t) = poetry.get(s) {
                poetry_tables.push((format!("tool.poetry.{s}"), t));
            }
        }
        if let Some(g) = poetry.get("group").and_then(|g| g.as_table()) {
            for (gn, gv) in g {
                if let Some(t) = gv.get("dependencies") {
                    poetry_tables.push((format!("tool.poetry.group.{gn}.dependencies"), t));
                }
            }
        }
    }
    for (sec, t) in poetry_tables {
        if let Some(tbl) = t.as_table() {
            for (k, val) in tbl {
                if k == "python" {
                    continue;
                }
                let checkable = !val
                    .as_table()
                    .map(|t| {
                        t.contains_key("path") || t.contains_key("git") || t.contains_key("url")
                    })
                    .unwrap_or(false);
                deps.push(DeclaredDep {
                    name: k.clone(),
                    registry_name: k.clone(),
                    section: sec.clone(),
                    checkable,
                    line: line_of(text, k),
                });
            }
        }
    }
    Some((name, deps))
}

fn parse_requirements(text: &str) -> Vec<DeclaredDep> {
    let mut out = vec![];
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with('-')
            || line.starts_with('.')
            || line.starts_with('/')
            || line.starts_with("git+")
            || line.starts_with("http")
            || line.starts_with("svn+")
            || line.starts_with("hg+")
        {
            continue;
        }
        if let Some((n, direct)) = parse_req(line) {
            let direct = direct || line.contains("://");
            out.push(DeclaredDep {
                name: n.clone(),
                registry_name: n,
                section: "requirements".into(),
                checkable: !direct,
                line: i as u32 + 1,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo() {
        let m = parse(
            "crates/a/Cargo.toml",
            "[package]\nname=\"a\"\n[dependencies]\nserde=\"1\"\nlocal={path=\"../b\"}\nrenamed={package=\"real-name\",version=\"1\"}\n[dev-dependencies]\ntempfile=\"3\"\n[target.'cfg(unix)'.dependencies]\nlibc=\"0.2\"\n",
        )
        .unwrap();
        assert_eq!(m.dir, "crates/a");
        assert_eq!(m.package_name.as_deref(), Some("a"));
        let get = |n: &str| m.deps.iter().find(|d| d.name == n).unwrap();
        assert!(get("serde").checkable);
        assert!(!get("local").checkable);
        assert_eq!(get("renamed").registry_name, "real-name");
        assert_eq!(get("tempfile").section, "dev-dependencies");
        assert!(m.deps.iter().any(|d| d.name == "libc"));
    }

    #[test]
    fn package_json() {
        let m = parse(
            "package.json",
            r#"{"name":"x","dependencies":{"a":"^1","loc":"file:../loc","al":"npm:real@^2","gh":"user/repo","@s/p":"1.0.0"},"devDependencies":{"jest":"^29"}}"#,
        )
        .unwrap();
        let get = |n: &str| m.deps.iter().find(|d| d.name == n).unwrap();
        assert!(get("a").checkable);
        assert!(!get("loc").checkable);
        assert_eq!(get("al").registry_name, "real");
        assert!(!get("gh").checkable);
        assert!(get("@s/p").checkable);
        assert_eq!(get("jest").section, "devDependencies");
    }

    #[test]
    fn pyproject_and_requirements() {
        let m = parse(
            "pyproject.toml",
            "[project]\nname=\"proj\"\ndependencies=[\"requests>=2\", \"flask[async]==3\", \"mine @ git+https://x/y.git\"]\n[project.optional-dependencies]\ndev=[\"pytest\"]\n[tool.poetry.dependencies]\npython=\"^3.9\"\nrich=\"^13\"\n",
        )
        .unwrap();
        let names: Vec<_> = m.deps.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"requests")
                && names.contains(&"flask")
                && names.contains(&"pytest")
                && names.contains(&"rich")
        );
        assert!(!names.contains(&"python"));
        assert!(!m.deps.iter().find(|d| d.name == "mine").unwrap().checkable);
        let r = parse("requirements-dev.txt", "# c\n-r base.txt\nnumpy==1.0  # x\n-e .\ngit+https://x/y.git\nfoo @ https://x/foo.zip\n").unwrap();
        let names: Vec<_> = r.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["numpy", "foo"]);
        assert!(!r.deps[1].checkable);
    }

    #[test]
    fn non_manifest() {
        assert!(parse("README.md", "x").is_none());
        assert!(parse("package.json", "{not json").is_none());
    }
}
