//! Self-Proving Verdict (SPV) foundation: types, Ed25519 manifests/receipts,
//! content-hash tree snapshots, policy floors, and full finding fingerprints.
//!
//! Phase 1 only. Mutation runner and dual-verdict integration come later.
//! Do not grade a scenario whose manifest signature was not verified (I5).
//! Callers may only tighten policy relative to floors (I6).

use crate::{Finding, Level};
use anyhow::{bail, Result};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

pub use ed25519_dalek::SigningKey;

/// Destination file pinned by content hash (sha256 of file bytes, lowercase hex).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Destination {
    pub canonical_path: PathBuf,
    /// Lowercase hex sha256 of file BYTES.
    pub content_hash: String,
}

/// Tool pin: path, content hash, version, and args hash (lowercase hex).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedTool {
    pub name: String,
    pub path: PathBuf,
    pub sha256: String,
    pub version: String,
    pub args_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AssertionKind {
    /// File content (read fresh) must contain needle.
    FileContains { file: String, needle: String },
    /// No fresh finding may match (rule, file).
    FindingAbsent { rule: String, file: String },
    /// Finding-free check: file content must contain the symbol text.
    SymbolPresent { file: String, symbol: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assertion {
    pub id: String,
    pub kind: AssertionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    pub enforce_dead_code: bool,
    pub dependencies: Level,
    pub weakening: Level,
    pub diagnostics: Level,
    pub rules: Level,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyFloors {
    pub enforce_dead_code: bool,
    pub dependencies: Level,
    pub weakening: Level,
    pub diagnostics: Level,
    pub rules: Level,
}

/// Manifest body that is signed. v1 canonical form is `serde_json::to_vec` of this
/// struct in declaration order; any field addition must bump `version`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestPayload {
    pub id: String,
    pub version: u32,
    pub destinations: Vec<Destination>,
    pub positive_assertions: Vec<Assertion>,
    pub negative_assertions: Vec<Assertion>,
    pub tools: Vec<PinnedTool>,
    pub policy: Policy,
}

/// Signed scenario manifest. `signature_hex` is 64-byte sig, `public_key_hex` is
/// 32-byte key, both lowercase hex.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScenarioManifest {
    pub payload: ManifestPayload,
    pub signature_hex: String,
    pub public_key_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvenanceTuple {
    pub finding_fingerprint_full: String,
    pub file_content_hash: String,
    pub tool_id: String,
    pub tool_version: String,
    pub tool_path_hash: String,
    pub args_hash: String,
    pub tree_id: String,
    /// RFC3339 UTC.
    pub executed_at: String,
}

/// Coverage over declared destinations. Non-empty `uncovered` means incomplete
/// (enforced in phase 3; struct only here).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoverageSet {
    pub covered: Vec<String>,
    pub uncovered: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CanaryResult {
    Caught { name: String },
    Missed { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpvVerdict {
    Pass,
    Fail,
    Incomplete,
    GateUntrusted,
}

/// Signed verdict receipt. Signature covers all fields except `signature_hex` /
/// `public_key_hex` (those are set by [`sign_receipt`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerdictReceipt {
    pub verdict: SpvVerdict,
    pub manifest_hash: String,
    pub tree_hash: String,
    pub coverage: CoverageSet,
    pub mutation_score: f32,
    pub canary_results: Vec<CanaryResult>,
    pub findings: Vec<ProvenanceTuple>,
    pub policy_hash: String,
    pub gate_binary_hash: String,
    pub signature_hex: String,
    pub public_key_hex: String,
}

/// Content-addressed tree snapshot. Entries are `(canonical rel path with '/', sha256 hex)`,
/// sorted by path. No mtime is read anywhere.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeSnapshot {
    pub files: Vec<(String, String)>,
    pub tree_hash: String,
}

/// Fields of [`VerdictReceipt`] included in the signed canonical bytes.
/// Excludes `signature_hex` and `public_key_hex`.
#[derive(Serialize)]
struct ReceiptCanon<'a> {
    verdict: &'a SpvVerdict,
    manifest_hash: &'a str,
    tree_hash: &'a str,
    coverage: &'a CoverageSet,
    mutation_score: f32,
    canary_results: &'a [CanaryResult],
    findings: &'a [ProvenanceTuple],
    policy_hash: &'a str,
    gate_binary_hash: &'a str,
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut a, b| {
        let _ = write!(a, "{b:02x}");
        a
    })
}

fn from_hex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("malformed hex: odd length");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => bail!("malformed hex: invalid digit"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    to_hex(&h.finalize())
}

/// v1 canonical signing bytes: `serde_json::to_vec` of the payload in field
/// declaration order. Any field addition must bump `ManifestPayload::version`.
pub fn canonical_bytes(payload: &ManifestPayload) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(payload)?)
}

fn receipt_canonical_bytes(receipt: &VerdictReceipt) -> Result<Vec<u8>> {
    let canon = ReceiptCanon {
        verdict: &receipt.verdict,
        manifest_hash: &receipt.manifest_hash,
        tree_hash: &receipt.tree_hash,
        coverage: &receipt.coverage,
        mutation_score: receipt.mutation_score,
        canary_results: &receipt.canary_results,
        findings: &receipt.findings,
        policy_hash: &receipt.policy_hash,
        gate_binary_hash: &receipt.gate_binary_hash,
    };
    Ok(serde_json::to_vec(&canon)?)
}

/// Sign a manifest payload; fills lowercase-hex signature and public key.
pub fn sign_manifest(
    payload: &ManifestPayload,
    signing_key: &SigningKey,
) -> Result<ScenarioManifest> {
    let bytes = canonical_bytes(payload)?;
    let sig = signing_key.sign(&bytes);
    let verifying = VerifyingKey::from(signing_key);
    Ok(ScenarioManifest {
        payload: payload.clone(),
        signature_hex: to_hex(&sig.to_bytes()),
        public_key_hex: to_hex(verifying.as_bytes()),
    })
}

/// Verify manifest signature and invariants. Fails (never Ok) on version != 1,
/// empty destinations, malformed hex, bad key/signature length, or bad signature.
/// Invariant I5: never grade a scenario whose manifest signature was not verified.
pub fn verify_manifest(manifest: &ScenarioManifest) -> Result<ManifestPayload> {
    if manifest.payload.version != 1 {
        bail!("manifest version must be 1");
    }
    if manifest.payload.destinations.is_empty() {
        bail!("manifest destinations must be non-empty");
    }
    let sig_bytes = from_hex(&manifest.signature_hex)?;
    if sig_bytes.len() != 64 {
        bail!("bad signature length: expected 64 bytes");
    }
    let key_bytes = from_hex(&manifest.public_key_hex)?;
    if key_bytes.len() != 32 {
        bail!("bad public key length: expected 32 bytes");
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);
    let signature = Signature::from_bytes(&sig_arr);
    let verifying =
        VerifyingKey::from_bytes(&key_arr).map_err(|e| anyhow::anyhow!("bad public key: {e}"))?;
    let bytes = canonical_bytes(&manifest.payload)?;
    verifying
        .verify(&bytes, &signature)
        .map_err(|e| anyhow::anyhow!("manifest signature verification failed: {e}"))?;
    Ok(manifest.payload.clone())
}

/// Hex sha256 of the canonical bytes of the manifest payload.
pub fn manifest_hash(manifest: &ScenarioManifest) -> String {
    match canonical_bytes(&manifest.payload) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex(&[]),
    }
}

/// Snapshot files under `root` by relative path. Rejects absolute paths, `..`
/// escapes, and symlinks. Reads file BYTES only (no mtime). Missing file = Err.
/// `tree_hash` = sha256 over `path + "\0" + hash + "\0"` for files sorted by path.
pub fn snapshot_tree(root: &Path, rel_paths: &[&str]) -> Result<TreeSnapshot> {
    let mut files: Vec<(String, String)> = Vec::with_capacity(rel_paths.len());
    for rel in rel_paths {
        validate_rel_path(rel)?;
        let full = root.join(rel);
        let meta = std::fs::symlink_metadata(&full)
            .map_err(|e| anyhow::anyhow!("missing or unreadable file {rel}: {e}"))?;
        if meta.file_type().is_symlink() {
            bail!("symlinks are not allowed in snapshot destinations: {rel}");
        }
        if !meta.is_file() {
            bail!("snapshot destination is not a regular file: {rel}");
        }
        let bytes =
            std::fs::read(&full).map_err(|e| anyhow::anyhow!("failed to read {rel}: {e}"))?;
        let hash = sha256_hex(&bytes);
        // Canonical rel path uses `/`.
        let canon = rel.replace('\\', "/");
        files.push((canon, hash));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut tree = Sha256::new();
    for (path, hash) in &files {
        tree.update(path.as_bytes());
        tree.update([0u8]);
        tree.update(hash.as_bytes());
        tree.update([0u8]);
    }
    Ok(TreeSnapshot {
        files,
        tree_hash: to_hex(&tree.finalize()),
    })
}

fn validate_rel_path(rel: &str) -> Result<()> {
    if rel.is_empty() {
        bail!("empty relative path");
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        bail!("absolute paths are not allowed: {rel}");
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => bail!("path escape (..) is not allowed: {rel}"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("absolute paths are not allowed: {rel}")
            }
        }
    }
    Ok(())
}

/// Floors matching [`crate::verify::VerifyPolicy`] defaults so a default policy passes.
pub fn default_floors() -> PolicyFloors {
    PolicyFloors {
        enforce_dead_code: false,
        dependencies: Level::Medium,
        weakening: Level::Medium,
        diagnostics: Level::Medium,
        rules: Level::Medium,
    }
}

/// Invariant I6: caller may only TIGHTEN relative to floors.
/// Level order is Low < Medium < High. Reject if any caller level is below the
/// floor, or if caller turns off `enforce_dead_code` while the floor requires it.
pub fn apply_policy(caller: &Policy, floors: &PolicyFloors) -> Result<Policy> {
    if floors.enforce_dead_code && !caller.enforce_dead_code {
        bail!("policy may not loosen enforce_dead_code below floor");
    }
    if caller.dependencies < floors.dependencies {
        bail!("policy may not loosen dependencies below floor");
    }
    if caller.weakening < floors.weakening {
        bail!("policy may not loosen weakening below floor");
    }
    if caller.diagnostics < floors.diagnostics {
        bail!("policy may not loosen diagnostics below floor");
    }
    if caller.rules < floors.rules {
        bail!("policy may not loosen rules below floor");
    }
    Ok(caller.clone())
}

/// Full 32-byte hex sha256 provenance fingerprint (64 hex chars).
/// Message is UN-normalised: no digit collapsing, no truncation.
/// Does not modify the lossy [`crate::verify::fingerprint`].
pub fn full_fingerprint(f: &Finding) -> String {
    let line = f.line.to_string();
    let column = f.column.map(|c| c.to_string()).unwrap_or_default();
    let symbol = f.symbol.clone().unwrap_or_default();
    let mut h = Sha256::new();
    for part in [
        f.category.as_str(),
        f.rule.as_str(),
        f.file.as_str(),
        line.as_str(),
        column.as_str(),
        symbol.as_str(),
        f.message.as_str(),
    ] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    to_hex(&h.finalize())
}

impl Assertion {
    /// Evaluate against fresh file contents and fresh findings.
    /// `files` are `(canonical rel path, content)` pairs. Missing file => false.
    pub fn check(&self, files: &[(String, String)], findings: &[Finding]) -> bool {
        match &self.kind {
            AssertionKind::FileContains { file, needle } => files
                .iter()
                .find(|(p, _)| p == file)
                .map(|(_, content)| content.contains(needle))
                .unwrap_or(false),
            AssertionKind::FindingAbsent { rule, file } => {
                !findings.iter().any(|f| f.rule == *rule && f.file == *file)
            }
            AssertionKind::SymbolPresent { file, symbol } => files
                .iter()
                .find(|(p, _)| p == file)
                .map(|(_, content)| content.contains(symbol))
                .unwrap_or(false),
        }
    }
}

/// Sign receipt canonical bytes (all fields except `signature_hex` /
/// `public_key_hex`, which this function sets).
pub fn sign_receipt(receipt: &mut VerdictReceipt, signing_key: &SigningKey) -> Result<()> {
    let bytes = receipt_canonical_bytes(receipt)?;
    let sig = signing_key.sign(&bytes);
    let verifying = VerifyingKey::from(signing_key);
    receipt.signature_hex = to_hex(&sig.to_bytes());
    receipt.public_key_hex = to_hex(verifying.as_bytes());
    Ok(())
}

/// Fail on bad or missing receipt signature.
pub fn verify_receipt(receipt: &VerdictReceipt) -> Result<()> {
    if receipt.signature_hex.is_empty() {
        bail!("missing receipt signature");
    }
    let sig_bytes = from_hex(&receipt.signature_hex)?;
    if sig_bytes.len() != 64 {
        bail!("bad receipt signature length: expected 64 bytes");
    }
    let key_bytes = from_hex(&receipt.public_key_hex)?;
    if key_bytes.len() != 32 {
        bail!("bad receipt public key length: expected 32 bytes");
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&key_bytes);
    let signature = Signature::from_bytes(&sig_arr);
    let verifying = VerifyingKey::from_bytes(&key_arr)
        .map_err(|e| anyhow::anyhow!("bad receipt public key: {e}"))?;
    let bytes = receipt_canonical_bytes(receipt)?;
    verifying
        .verify(&bytes, &signature)
        .map_err(|e| anyhow::anyhow!("receipt signature verification failed: {e}"))?;
    Ok(())
}

/// Hex sha256 of canonical receipt bytes (excluding signature fields).
pub fn receipt_hash(receipt: &VerdictReceipt) -> String {
    match receipt_canonical_bytes(receipt) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex(&[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_key_a() -> SigningKey {
        SigningKey::from_bytes(&[1u8; 32])
    }

    fn test_key_b() -> SigningKey {
        SigningKey::from_bytes(&[2u8; 32])
    }

    fn sample_payload() -> ManifestPayload {
        ManifestPayload {
            id: "scenario-1".into(),
            version: 1,
            destinations: vec![Destination {
                canonical_path: PathBuf::from("src/main.rs"),
                content_hash: "aabb".into(),
            }],
            positive_assertions: vec![],
            negative_assertions: vec![],
            tools: vec![],
            policy: Policy {
                enforce_dead_code: false,
                dependencies: Level::Medium,
                weakening: Level::Medium,
                diagnostics: Level::Medium,
                rules: Level::Medium,
            },
        }
    }

    fn sample_receipt() -> VerdictReceipt {
        VerdictReceipt {
            verdict: SpvVerdict::Pass,
            manifest_hash: "m".into(),
            tree_hash: "t".into(),
            coverage: CoverageSet {
                covered: vec!["a".into()],
                uncovered: vec![],
            },
            mutation_score: 1.0,
            canary_results: vec![CanaryResult::Caught { name: "c1".into() }],
            findings: vec![],
            policy_hash: "p".into(),
            gate_binary_hash: "g".into(),
            signature_hex: String::new(),
            public_key_hex: String::new(),
        }
    }

    #[test]
    fn manifest_sign_verify_roundtrip() {
        let payload = sample_payload();
        let key = test_key_a();
        let manifest = sign_manifest(&payload, &key).expect("sign");
        let got = verify_manifest(&manifest).expect("verify");
        assert_eq!(got, payload);
    }

    #[test]
    fn tampered_manifest_refused() {
        let key = test_key_a();
        let mut manifest = sign_manifest(&sample_payload(), &key).expect("sign");
        manifest.payload.destinations[0].content_hash = "ffff".into();
        assert!(verify_manifest(&manifest).is_err());
    }

    #[test]
    fn wrong_key_refused() {
        let key_a = test_key_a();
        let key_b = test_key_b();
        let mut manifest = sign_manifest(&sample_payload(), &key_a).expect("sign");
        let vk_b = VerifyingKey::from(&key_b);
        manifest.public_key_hex = to_hex(vk_b.as_bytes());
        assert!(verify_manifest(&manifest).is_err());
    }

    #[test]
    fn missing_signature_refused() {
        let key = test_key_a();
        let mut manifest = sign_manifest(&sample_payload(), &key).expect("sign");

        manifest.signature_hex.clear();
        assert!(verify_manifest(&manifest).is_err());

        manifest = sign_manifest(&sample_payload(), &key).expect("sign");
        manifest.signature_hex = "zz".into();
        assert!(verify_manifest(&manifest).is_err());

        manifest = sign_manifest(&sample_payload(), &key).expect("sign");
        // 31-byte key (62 hex chars).
        manifest.public_key_hex = to_hex(&[0u8; 31]);
        assert!(verify_manifest(&manifest).is_err());
    }

    #[test]
    fn version_check() {
        let key = test_key_a();
        let mut payload = sample_payload();
        payload.version = 2;
        let manifest = sign_manifest(&payload, &key).expect("sign");
        assert!(verify_manifest(&manifest).is_err());
    }

    #[test]
    fn empty_destinations_refused() {
        let key = test_key_a();
        let mut payload = sample_payload();
        payload.destinations.clear();
        let manifest = sign_manifest(&payload, &key).expect("sign");
        assert!(verify_manifest(&manifest).is_err());
    }

    #[test]
    fn snapshot_detects_content_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rel = "file.txt";
        let path = dir.path().join(rel);
        fs::write(&path, b"hello").expect("write");
        let snap1 = snapshot_tree(dir.path(), &[rel]).expect("snap1");
        fs::write(&path, b"hellp").expect("change");
        let snap2 = snapshot_tree(dir.path(), &[rel]).expect("snap2");
        assert_ne!(snap1.files[0].1, snap2.files[0].1);
        assert_ne!(snap1.tree_hash, snap2.tree_hash);
    }

    #[test]
    fn snapshot_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.txt"), b"a").expect("write a");
        fs::write(dir.path().join("b.txt"), b"b").expect("write b");
        let paths = ["a.txt", "b.txt"];
        let s1 = snapshot_tree(dir.path(), &paths).expect("s1");
        let s2 = snapshot_tree(dir.path(), &paths).expect("s2");
        assert_eq!(s1, s2);
    }

    #[test]
    fn snapshot_rejects_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("ok.txt"), b"x").expect("write");
        assert!(snapshot_tree(dir.path(), &["../outside.txt"]).is_err());
        assert!(snapshot_tree(dir.path(), &["/absolute.txt"]).is_err());
        assert!(snapshot_tree(dir.path(), &["missing.txt"]).is_err());
    }

    #[test]
    fn policy_floor_rejects_loosening() {
        let floors = default_floors();
        let mut caller = Policy {
            enforce_dead_code: false,
            dependencies: Level::Medium,
            weakening: Level::Medium,
            diagnostics: Level::Low,
            rules: Level::Medium,
        };
        assert!(apply_policy(&caller, &floors).is_err());
        caller.diagnostics = Level::High;
        assert!(apply_policy(&caller, &floors).is_ok());
    }

    #[test]
    fn policy_tighten_only() {
        let floors = PolicyFloors {
            enforce_dead_code: false,
            dependencies: Level::Medium,
            weakening: Level::Medium,
            diagnostics: Level::Medium,
            rules: Level::High,
        };
        let caller = Policy {
            enforce_dead_code: false,
            dependencies: Level::High, // tighten
            weakening: Level::Medium,
            diagnostics: Level::Medium,
            rules: Level::Low, // loosen one category
        };
        assert!(apply_policy(&caller, &floors).is_err());
    }

    #[test]
    fn full_fingerprint_distinguishes_lines() {
        let mut a = Finding::new("cat", "rule", "f.rs", 10, 0.9, "src", "msg 1".into());
        let mut b = a.clone();
        b.line = 11;
        let fa = full_fingerprint(&a);
        let fb = full_fingerprint(&b);
        assert_ne!(fa, fb);
        assert_eq!(fa.len(), 64);
        assert_eq!(fb.len(), 64);
        a.symbol = Some("sym".into());
        b.symbol = Some("sym".into());
        let truncated_a = crate::verify::fingerprint(&a);
        let truncated_b = crate::verify::fingerprint(&b);
        // Existing fingerprint ignores line numbers; full does not.
        assert_eq!(truncated_a, truncated_b);
        assert_ne!(full_fingerprint(&a), truncated_a);
        assert_ne!(full_fingerprint(&b), truncated_b);
    }

    #[test]
    fn receipt_sign_verify_roundtrip() {
        let key = test_key_a();
        let mut receipt = sample_receipt();
        sign_receipt(&mut receipt, &key).expect("sign");
        verify_receipt(&receipt).expect("verify");
    }

    #[test]
    fn tampered_receipt_refused() {
        let key = test_key_a();
        let mut receipt = sample_receipt();
        sign_receipt(&mut receipt, &key).expect("sign");
        receipt.verdict = SpvVerdict::Fail;
        assert!(verify_receipt(&receipt).is_err());
    }

    #[test]
    fn assertion_checks() {
        let files = vec![
            ("src/a.rs".into(), "fn foo() { bar }".into()),
            ("src/b.rs".into(), "hello".into()),
        ];
        let findings = vec![Finding::new(
            "dead",
            "unused",
            "src/a.rs",
            1,
            0.9,
            "t0",
            "unused foo".into(),
        )];

        let ok_contains = Assertion {
            id: "1".into(),
            kind: AssertionKind::FileContains {
                file: "src/a.rs".into(),
                needle: "foo".into(),
            },
        };
        assert!(ok_contains.check(&files, &findings));

        let bad_contains = Assertion {
            id: "2".into(),
            kind: AssertionKind::FileContains {
                file: "src/a.rs".into(),
                needle: "missing".into(),
            },
        };
        assert!(!bad_contains.check(&files, &findings));

        let missing_file = Assertion {
            id: "3".into(),
            kind: AssertionKind::FileContains {
                file: "nope.rs".into(),
                needle: "x".into(),
            },
        };
        assert!(!missing_file.check(&files, &findings));

        let absent_ok = Assertion {
            id: "4".into(),
            kind: AssertionKind::FindingAbsent {
                rule: "other".into(),
                file: "src/a.rs".into(),
            },
        };
        assert!(absent_ok.check(&files, &findings));

        let absent_fail = Assertion {
            id: "5".into(),
            kind: AssertionKind::FindingAbsent {
                rule: "unused".into(),
                file: "src/a.rs".into(),
            },
        };
        assert!(!absent_fail.check(&files, &findings));

        let sym_ok = Assertion {
            id: "6".into(),
            kind: AssertionKind::SymbolPresent {
                file: "src/a.rs".into(),
                symbol: "bar".into(),
            },
        };
        assert!(sym_ok.check(&files, &findings));

        let sym_fail = Assertion {
            id: "7".into(),
            kind: AssertionKind::SymbolPresent {
                file: "src/a.rs".into(),
                symbol: "baz".into(),
            },
        };
        assert!(!sym_fail.check(&files, &findings));
    }

    #[test]
    fn canonical_bytes_deterministic() {
        let payload = sample_payload();
        let a = canonical_bytes(&payload).expect("a");
        let b = canonical_bytes(&payload).expect("b");
        assert_eq!(a, b);
    }
}
