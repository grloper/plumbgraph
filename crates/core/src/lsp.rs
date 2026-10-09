//! Minimal stdio LSP client that pulls `textDocument/publishDiagnostics`.
//!
//! It starts the given language server (**executing the project's toolchain**, so callers
//! must treat this as an explicit opt-in), opens the files, waits until every file has been
//! reported on and the server has been quiet for a moment (or the timeout elapses), then shuts
//! the server down. Servers that compute diagnostics lazily (e.g. run `cargo check` in the
//! background) may need a larger timeout.

use crate::diagnostics::{clean_message, confidence_for, make, DiagReport, ToolRun};
use crate::{sanitize, Severity};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Quiet period after all files were reported before we stop waiting.
const SETTLE: Duration = Duration::from_millis(1200);
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

fn language_id(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        _ => "plaintext",
    }
}

fn path_to_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                s.push(b as char)
            }
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

fn uri_to_path(u: &str) -> Option<String> {
    let rest = u.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn read_message<R: BufRead>(r: &mut R) -> Option<Value> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().ok();
            }
        }
    }
    let n = len?;
    if n > MAX_MESSAGE {
        return None;
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_message<W: Write>(w: &mut W, v: &Value) -> Result<()> {
    let b = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", b.len())?;
    w.write_all(&b)?;
    w.flush()?;
    Ok(())
}

fn severity_of(v: &Value) -> Severity {
    match v.as_u64() {
        Some(1) => Severity::Error,
        Some(2) => Severity::Warning,
        _ => Severity::Info,
    }
}

/// Start `server`, open `files`, and return the diagnostics it publishes.
pub fn collect(
    server: &[String],
    root: &Path,
    files: &[PathBuf],
    timeout: Duration,
) -> Result<DiagReport> {
    let root = root
        .canonicalize()
        .with_context(|| format!("root {}", root.display()))?;
    let (prog, args) = server
        .split_first()
        .ok_or_else(|| anyhow!("empty LSP server command"))?;
    let mut opened: Vec<(PathBuf, String)> = vec![];
    for f in files {
        let abs = if f.is_absolute() {
            f.clone()
        } else {
            root.join(f)
        };
        let abs = abs
            .canonicalize()
            .with_context(|| format!("file {}", sanitize(&f.display().to_string())))?;
        if !abs.starts_with(&root) {
            bail!(
                "{} is outside the project root",
                sanitize(&abs.display().to_string())
            );
        }
        opened.push((abs.clone(), path_to_uri(&abs)));
    }
    let mut child = Command::new(prog)
        .args(args)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting LSP server `{}`", sanitize(prog)))?;
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        while let Some(m) = read_message(&mut r) {
            if tx.send(m).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let result = (|| -> Result<HashMap<String, Vec<Value>>> {
        let root_uri = path_to_uri(&root);
        write_message(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": "root"}],
                "clientInfo": {"name": "plumbgraph"},
                "capabilities": {
                    "textDocument": {"publishDiagnostics": {"relatedInformation": false}},
                    "window": {"workDoneProgress": true},
                    "experimental": {"serverStatusNotification": true}
                }
            }}),
        )?;
        let mut published: HashMap<String, Vec<Value>> = HashMap::new();
        let mut initialized = false;
        let mut last_activity = Instant::now();
        let mut sent_open = false;
        // work-done progress tokens that began and have not ended: the server is still loading
        let mut busy: std::collections::HashSet<String> = std::collections::HashSet::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                if !initialized {
                    bail!(
                        "LSP server did not answer `initialize` within {} ms",
                        timeout.as_millis()
                    );
                }
                break;
            }
            if sent_open
                && busy.is_empty()
                && opened.iter().all(|(_, u)| published.contains_key(u))
                && now.duration_since(last_activity) >= SETTLE
            {
                break;
            }
            let wait = (deadline - now).min(Duration::from_millis(100));
            let msg = match rx.recv_timeout(wait) {
                Ok(m) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if !initialized {
                        bail!("LSP server exited before answering `initialize`");
                    }
                    break;
                }
            };
            last_activity = Instant::now();
            let method = msg.get("method").and_then(|m| m.as_str());
            let id = msg.get("id");
            match (method, id) {
                (None, Some(i)) if i == &json!(1) => {
                    if let Some(e) = msg.get("error") {
                        bail!("LSP initialize failed: {}", sanitize(&e.to_string()));
                    }
                    initialized = true;
                    write_message(
                        &mut stdin,
                        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
                    )?;
                    for (p, uri) in &opened {
                        let md = std::fs::metadata(p)?;
                        if md.len() > MAX_FILE_BYTES {
                            bail!(
                                "{} is larger than 2 MiB",
                                sanitize(&p.display().to_string())
                            );
                        }
                        let text = std::fs::read_to_string(p).with_context(|| {
                            format!("reading {}", sanitize(&p.display().to_string()))
                        })?;
                        write_message(
                            &mut stdin,
                            &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{
                                "uri": uri, "languageId": language_id(p), "version": 1, "text": text}}}),
                        )?;
                    }
                    sent_open = true;
                }
                (Some("$/progress"), None) => {
                    let tok = msg["params"]["token"].to_string();
                    match msg["params"]["value"]["kind"].as_str() {
                        Some("begin") => {
                            busy.insert(tok);
                        }
                        Some("end") => {
                            busy.remove(&tok);
                        }
                        _ => {}
                    }
                }
                (Some("experimental/serverStatus"), None) => {
                    // rust-analyzer: `quiescent = false` while it loads / indexes
                    if msg["params"]["quiescent"] == json!(false) {
                        busy.insert("serverStatus".into());
                    } else {
                        busy.remove("serverStatus");
                    }
                }
                (Some("textDocument/publishDiagnostics"), None) => {
                    let p = &msg["params"];
                    if let (Some(uri), Some(d)) = (p["uri"].as_str(), p["diagnostics"].as_array()) {
                        published.insert(uri.to_string(), d.clone());
                    }
                }
                // server -> client requests must be answered or some servers stall
                (Some(m), Some(i)) => {
                    let result = if m == "workspace/configuration" {
                        let n = msg["params"]["items"]
                            .as_array()
                            .map(|a| a.len())
                            .unwrap_or(0);
                        Value::Array(vec![Value::Null; n])
                    } else {
                        Value::Null
                    };
                    write_message(&mut stdin, &json!({"jsonrpc":"2.0","id":i,"result":result}))?;
                }
                _ => {}
            }
        }
        // orderly shutdown (bounded)
        let _ = write_message(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        );
        let until = Instant::now() + Duration::from_millis(800);
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(m) if m.get("id") == Some(&json!(2)) => break,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        let _ = write_message(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"}));
        Ok(published)
    })();
    drop(stdin);
    std::thread::sleep(Duration::from_millis(50));
    let _ = child.kill();
    let _ = child.wait();
    let published = result?;

    // `rustup run stable rust-analyzer`, `npx pyright-langserver --stdio`, `python3 server.py`:
    // name the server, not the launcher.
    const LAUNCHERS: &[&str] = &[
        "rustup", "run", "stable", "nightly", "env", "npx", "node", "python", "python3", "uv",
        "uvx", "exec",
    ];
    let name = server
        .iter()
        .map(|a| {
            Path::new(a)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| a.clone())
        })
        .find(|n| !n.starts_with('-') && !LAUNCHERS.contains(&n.as_str()))
        .unwrap_or_else(|| prog.clone());
    let source = format!("lsp:{}", sanitize(&name));
    let mut rep = DiagReport {
        executed: true,
        ..Default::default()
    };
    let mut all = vec![];
    for (uri, diags) in &published {
        let Some(path) = uri_to_path(uri) else {
            continue;
        };
        let Some(rel) = crate::diagnostics::rel_path(&root, &path) else {
            rep.dropped_outside_root += diags.len();
            continue;
        };
        for d in diags {
            let sev = severity_of(&d["severity"]);
            let rule = match &d["code"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => d["source"].as_str().unwrap_or("lsp").to_string(),
            };
            let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) as u32 + 1;
            let col = d["range"]["start"]["character"].as_u64().unwrap_or(0) as u32 + 1;
            let mut f = make(
                "lsp",
                &source,
                &rule,
                &rel,
                line,
                Some(col),
                sev,
                d["message"].as_str().unwrap_or(""),
            );
            f.confidence = (confidence_for("lsp", sev) * 100.0).round() / 100.0;
            if let Some(s) = d["source"].as_str() {
                f.evidence.push(format!(
                    "language-server diagnostic source: {}",
                    clean_message(s)
                ));
            }
            all.push(f);
        }
    }
    rep.tools.push(ToolRun {
        tool: source.clone(),
        command: sanitize(&server.join(" ")),
        status: "ran".into(),
        findings: all.len(),
        note: None,
    });
    rep.findings = crate::diagnostics::merge(vec![DiagReport {
        findings: all,
        ..Default::default()
    }])
    .findings;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip_with_spaces_and_unicode() {
        let p = Path::new("/tmp/my proj/ü file.rs");
        let u = path_to_uri(p);
        assert_eq!(u, "file:///tmp/my%20proj/%C3%BC%20file.rs");
        assert_eq!(uri_to_path(&u).as_deref(), Some("/tmp/my proj/ü file.rs"));
        assert_eq!(uri_to_path("http://x"), None);
        assert_eq!(uri_to_path("file:///a%2"), Some("/a%2".to_string()));
    }

    #[test]
    fn framing_roundtrip_and_oversize_rejected() {
        let mut buf = vec![];
        write_message(&mut buf, &json!({"a": 1})).unwrap();
        let mut r = BufReader::new(&buf[..]);
        assert_eq!(read_message(&mut r), Some(json!({"a": 1})));
        let big = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1);
        assert_eq!(read_message(&mut BufReader::new(big.as_bytes())), None);
    }
}
