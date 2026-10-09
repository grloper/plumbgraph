//! Plumbgraph core: code-graph model, tree-sitter extraction, SQLite store and the
//! checks built on top (dead code, dependency/import checks, test-weakening).
//!
//! Every [`Finding`] carries a `source` (which analysis produced it) and a numeric
//! `confidence` with a coarse [`Level`].

pub mod deadcode;
pub mod deps;
pub mod diagnostics;
pub mod extract;
pub mod index;
pub mod init;
pub mod lsp;
pub mod manifest;
pub mod map;
pub mod model;
pub mod ops;
pub mod providers;
pub mod registry;
pub mod resolve;
pub mod rules;
pub mod scip;
pub mod store;
pub mod verify;
pub mod weakening;

use serde::{Deserialize, Serialize};

/// Coarse confidence level derived from the numeric score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Low,
    Medium,
    High,
}

impl Level {
    /// high >= 0.90, medium >= 0.70, otherwise low.
    pub fn from_confidence(c: f64) -> Self {
        if c >= 0.90 {
            Level::High
        } else if c >= 0.70 {
            Level::Medium
        } else {
            Level::Low
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A single result of any check. `source` says where the evidence came from
/// (e.g. `t0-treesitter`, `manifest`, `registry:pypi`, `git-diff`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub category: String,
    pub rule: String,
    pub file: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// 1-based column, when the source provides one (diagnostics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub kind: String,
    pub severity: Severity,
    pub confidence: f64,
    pub level: Level,
    pub source: String,
    pub message: String,
    pub evidence: Vec<String>,
    pub fp_risks: Vec<String>,
}

impl Finding {
    pub fn new(
        category: &str,
        rule: &str,
        file: &str,
        line: u32,
        confidence: f64,
        source: &str,
        message: String,
    ) -> Self {
        let confidence = (confidence * 100.0).round() / 100.0;
        Finding {
            category: category.into(),
            rule: rule.into(),
            file: file.into(),
            line,
            symbol: None,
            column: None,
            kind: String::new(),
            severity: Severity::Warning,
            confidence,
            level: Level::from_confidence(confidence),
            source: source.into(),
            message,
            evidence: vec![],
            fp_risks: vec![],
        }
    }
}

/// Strip control characters (incl. ANSI escapes / newlines) and cap length. Used for
/// every string derived from repository contents before it is shown to a human or
/// an agent.
pub fn sanitize(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    if cleaned.chars().count() > 200 {
        let mut t: String = cleaned.chars().take(200).collect();
        t.push('…');
        t
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(Level::from_confidence(0.95), Level::High);
        assert_eq!(Level::from_confidence(0.90), Level::High);
        assert_eq!(Level::from_confidence(0.89), Level::Medium);
        assert_eq!(Level::from_confidence(0.70), Level::Medium);
        assert_eq!(Level::from_confidence(0.69), Level::Low);
    }

    #[test]
    fn sanitize_strips_controls() {
        assert_eq!(sanitize("a\x1b[31mb\nc"), "a[31mbc");
        assert!(sanitize(&"x".repeat(500)).chars().count() <= 201);
    }
}
