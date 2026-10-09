//! Read-only registry existence lookups (PyPI, npm, crates.io) with an on-disk cache
//! and an offline mode. Only the package *name* is sent; nothing else leaves the machine.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existence {
    Exists,
    NotFound,
    /// Network error, rate limit, offline, invalid name... We never claim "nonexistent" from this.
    Unknown(String),
}

pub trait Registry: Sync {
    fn lookup(&self, eco: &str, name: &str) -> Existence;
    fn name(&self) -> &'static str;
}

/// Never touches the network.
pub struct Offline;
impl Registry for Offline {
    fn lookup(&self, _eco: &str, _name: &str) -> Existence {
        Existence::Unknown("offline mode".into())
    }
    fn name(&self) -> &'static str {
        "offline"
    }
}

/// Fixed answers (used by tests).
pub struct StaticRegistry {
    pub known: HashMap<(String, String), bool>,
}
impl Registry for StaticRegistry {
    fn lookup(&self, eco: &str, name: &str) -> Existence {
        match self.known.get(&(eco.to_string(), name.to_string())) {
            Some(true) => Existence::Exists,
            Some(false) => Existence::NotFound,
            None => Existence::Unknown("not in static table".into()),
        }
    }
    fn name(&self) -> &'static str {
        "static"
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct CacheEntry {
    exists: bool,
    fetched_at: u64,
}

pub struct HttpRegistry {
    agent: ureq::Agent,
    cache_path: Option<PathBuf>,
    cache: Mutex<HashMap<String, CacheEntry>>,
    pub ttl_exists: Duration,
    pub ttl_missing: Duration,
    base: Bases,
}

#[derive(Clone)]
struct Bases {
    pypi: String,
    npm: String,
    crates: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate a package name for an ecosystem; invalid names are never sent over the network.
pub fn valid_name(eco: &str, name: &str) -> bool {
    let ok = |c: char, extra: &str| c.is_ascii_alphanumeric() || extra.contains(c);
    match eco {
        "pypi" => {
            !name.is_empty()
                && name.len() <= 100
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphanumeric())
                    .unwrap_or(false)
                && name.chars().all(|c| ok(c, "._-"))
        }
        "crates.io" => {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphabetic())
                    .unwrap_or(false)
                && name.chars().all(|c| ok(c, "_-"))
        }
        "npm" => {
            if name.is_empty() || name.len() > 214 {
                return false;
            }
            let seg_ok = |s: &str| {
                !s.is_empty() && !s.starts_with(['.', '_']) && s.chars().all(|c| ok(c, "._~-"))
            };
            match name.strip_prefix('@') {
                Some(rest) => match rest.split_once('/') {
                    Some((scope, pkg)) => seg_ok(scope) && seg_ok(pkg) && !pkg.contains('/'),
                    None => false,
                },
                None => seg_ok(name),
            }
        }
        _ => false,
    }
}

impl HttpRegistry {
    pub fn new(cache_path: Option<PathBuf>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!(
                "plumbgraph/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/grloper/plumbgraph; existence check)"
            ))
            .build();
        let cache = cache_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        HttpRegistry {
            agent,
            cache_path,
            cache: Mutex::new(cache),
            ttl_exists: Duration::from_secs(24 * 3600),
            ttl_missing: Duration::from_secs(3600),
            base: Bases {
                pypi: "https://pypi.org/pypi".into(),
                npm: "https://registry.npmjs.org".into(),
                crates: "https://crates.io/api/v1/crates".into(),
            },
        }
    }

    /// Point at a different server (tests use a local mock).
    pub fn with_bases(mut self, pypi: &str, npm: &str, crates: &str) -> Self {
        self.base = Bases {
            pypi: pypi.into(),
            npm: npm.into(),
            crates: crates.into(),
        };
        self
    }

    pub fn save_cache(&self) {
        if let Some(p) = &self.cache_path {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Ok(c) = self.cache.lock() {
                if let Ok(t) = serde_json::to_string(&*c) {
                    let _ = std::fs::write(p, t);
                }
            }
        }
    }

    fn url(&self, eco: &str, name: &str) -> String {
        match eco {
            "pypi" => format!("{}/{}/json", self.base.pypi, name),
            "npm" => format!("{}/{}", self.base.npm, name.replace('/', "%2F")),
            _ => format!("{}/{}", self.base.crates, name),
        }
    }
}

impl Registry for HttpRegistry {
    fn lookup(&self, eco: &str, name: &str) -> Existence {
        if !valid_name(eco, name) {
            return Existence::Unknown("invalid package name for this ecosystem".into());
        }
        let key = format!("{eco}:{}", name.to_ascii_lowercase());
        if let Ok(c) = self.cache.lock() {
            if let Some(e) = c.get(&key) {
                let ttl = if e.exists {
                    self.ttl_exists
                } else {
                    self.ttl_missing
                };
                if now().saturating_sub(e.fetched_at) < ttl.as_secs() {
                    return if e.exists {
                        Existence::Exists
                    } else {
                        Existence::NotFound
                    };
                }
            }
        }
        let url = self.url(eco, name);
        let res = self.agent.get(&url).call();
        let out = match res {
            Ok(r) if r.status() == 200 => Existence::Exists,
            Ok(r) => Existence::Unknown(format!("unexpected HTTP status {}", r.status())),
            Err(ureq::Error::Status(404, _)) => Existence::NotFound,
            Err(ureq::Error::Status(code, _)) => Existence::Unknown(format!("HTTP {code}")),
            Err(e) => Existence::Unknown(format!("network error: {}", e.kind())),
        };
        if matches!(out, Existence::Exists | Existence::NotFound) {
            if let Ok(mut c) = self.cache.lock() {
                c.insert(
                    key,
                    CacheEntry {
                        exists: out == Existence::Exists,
                        fetched_at: now(),
                    },
                );
            }
        }
        out
    }
    fn name(&self) -> &'static str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn name_validation() {
        assert!(valid_name("pypi", "requests"));
        assert!(valid_name("pypi", "zope.interface"));
        assert!(!valid_name("pypi", "../etc/passwd"));
        assert!(!valid_name("pypi", "a b"));
        assert!(valid_name("npm", "left-pad"));
        assert!(valid_name("npm", "@scope/pkg"));
        assert!(!valid_name("npm", "@scope"));
        assert!(!valid_name("npm", "a/b"));
        assert!(!valid_name("npm", "pkg?x=1"));
        assert!(valid_name("crates.io", "serde_json"));
        assert!(!valid_name("crates.io", "1abc"));
        assert!(!valid_name("nuget", "x"));
    }

    #[test]
    fn offline_and_static() {
        assert!(matches!(Offline.lookup("npm", "x"), Existence::Unknown(_)));
        let mut known = HashMap::new();
        known.insert(("npm".to_string(), "real".to_string()), true);
        known.insert(("npm".to_string(), "fake".to_string()), false);
        let r = StaticRegistry { known };
        assert_eq!(r.lookup("npm", "real"), Existence::Exists);
        assert_eq!(r.lookup("npm", "fake"), Existence::NotFound);
        assert!(matches!(r.lookup("npm", "other"), Existence::Unknown(_)));
    }

    /// Spin up a tiny local HTTP server: 200 for names starting with "real", 404 for "missing", 500 otherwise.
    fn mock_server() -> (
        String,
        std::thread::JoinHandle<()>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h2 = hits.clone();
        let t = std::thread::spawn(move || {
            for _ in 0..3 {
                let Ok((mut s, _)) = l.accept() else { break };
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                h2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let path = req.split_whitespace().nth(1).unwrap_or("");
                let code = if path.contains("real") {
                    "200 OK"
                } else if path.contains("missing") {
                    "404 Not Found"
                } else {
                    "500 Internal Server Error"
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {code}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
            }
        });
        (format!("http://{addr}"), t, hits)
    }

    #[test]
    fn http_lookup_status_mapping_and_cache() {
        let (base, t, hits) = mock_server();
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("c.json");
        let r = HttpRegistry::new(Some(cache.clone())).with_bases(&base, &base, &base);
        assert_eq!(r.lookup("npm", "realpkg"), Existence::Exists);
        assert_eq!(r.lookup("npm", "missingpkg"), Existence::NotFound);
        assert!(
            matches!(r.lookup("npm", "boom"), Existence::Unknown(_)),
            "5xx must never be reported as nonexistent"
        );
        // cached: no extra hits for the first two
        assert_eq!(r.lookup("npm", "realpkg"), Existence::Exists);
        assert_eq!(r.lookup("npm", "missingpkg"), Existence::NotFound);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 3);
        r.save_cache();
        assert!(cache.exists());
        let _ = t.join();
    }
}
