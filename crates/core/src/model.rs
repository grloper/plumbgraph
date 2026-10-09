//! Plain data types describing what extraction finds in a single file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolFact {
    pub name: String,
    pub qname: String,
    /// function | method | class | struct | enum | trait | interface | type | const | variable | module | macro
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: usize,
    pub end_byte: usize,
    pub exported: bool,
    pub is_test: bool,
    /// Why this symbol is an entry point (None = not an entry point).
    pub entry: Option<String>,
    /// When set, the entry only counts if the condition holds: `parent` or `type:<Name>`.
    pub entry_cond: Option<String>,
    /// Carries a framework-style decorator / attribute (lowers dead-code confidence).
    pub decorated: bool,
    /// Method of a class that extends/implements another type (may override a base/framework method).
    pub subclass_method: bool,
    /// `plumb:keep(reason)` annotation text.
    pub keep: Option<String>,
    /// Index of the lexically enclosing symbol in the same file.
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RefFact {
    pub name: String,
    /// `calls` or `references`
    pub kind: String,
    pub line: u32,
    /// Accessed as `x.name`, `Type::name` etc.
    pub member: bool,
    /// Index of the innermost enclosing symbol; None = module-level code.
    pub src: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportFact {
    pub module: String,
    pub names: Vec<String>,
    pub line: u32,
    /// `import` | `mod` | `require` | `extern_crate`
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileFacts {
    pub lang: String,
    pub is_test: bool,
    pub is_generated: bool,
    pub has_syntax_errors: bool,
    pub dynamic_markers: Vec<String>,
    pub symbols: Vec<SymbolFact>,
    pub refs: Vec<RefFact>,
    pub imports: Vec<ImportFact>,
    pub strings: BTreeSet<String>,
}
