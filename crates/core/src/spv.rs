//! Self-Proving Verdict (SPV): types, Ed25519 manifests/receipts, content-hash
//! tree snapshots, policy floors, full finding fingerprints, scenario grading,
//! the self-test mutation runner (phase 2), dual independent verdict, and the
//! `run_spv` entry point with signed receipt emission (phase 3).
//!
//! Do not grade a scenario whose manifest signature was not verified (I5).
//! Callers may only tighten policy relative to floors (I6).
//! The gate must catch every derived mutation and canary before grading anyone.
//! Dual verdict disagreement yields Incomplete (I9). Self-test runs first (I1).
//!
//! Known v1 limitation (I4): baseline entries are never dropped as fixed.
//! I4 (a baseline entry may drop as fixed only on a fresh read of its
//! destination) is not yet enforced; v1 leaves baselines untouched.

use crate::{Finding, Level};
use anyhow::{bail, Result};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// True only when primary and independent dual verdicts disagreed (I9).
    pub verdict_disagreement: bool,
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
    verdict_disagreement: bool,
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
        verdict_disagreement: receipt.verdict_disagreement,
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

/// Fresh evidence produced by pinned-tool executions at grading time.
///
/// Phase 3 supplies a production provider; phase 2 uses only a test fake.
pub trait EvidenceProvider {
    /// Fresh findings produced by pinned-tool executions covering the declared destinations.
    fn fresh_findings(&self) -> &[Finding];
    /// Fresh file content for a canonical rel path (read by the provider at grading time).
    fn file_content(&self, rel_path: &str) -> Option<String>;
    /// Which declared destinations the provider's tools actually covered.
    fn covered_destinations(&self) -> &[String];
    /// False if any finding came from saved input rather than a fresh execution (invariant I3).
    fn is_fresh(&self) -> bool;
    /// Tool attribution for a finding: `(tool_id, tool_version, tool_path_hash, args_hash)`.
    /// `None` if the finding cannot be attributed to a pinned-tool execution
    /// (unattributed findings are listed with empty tool fields and never count
    /// as verified-absent evidence).
    fn tool_attribution(&self, finding: &Finding) -> Option<(String, String, String, String)>;
}

fn destination_rel(dest: &Destination) -> String {
    dest.canonical_path.to_string_lossy().replace('\\', "/")
}

/// Grade a signed scenario against fresh evidence.
///
/// Verdict precedence (highest wins): `GateUntrusted` > `Fail` > `Incomplete` > `Pass`.
/// Steps are deterministic and none are skippable:
/// 1. Manifest signature (I5) — fail => `GateUntrusted`
/// 2. Policy floors (I6) — fail => `GateUntrusted`
/// 3. Tree snapshot — fail => `Incomplete`
/// 4. Snapshot vs manifest content hashes — mismatch => `Incomplete`
/// 5. Coverage (I7) — uncovered destination => `Incomplete`
/// 6. Freshness (I3) — non-fresh => `Incomplete` (before assertions; never Pass/Fail)
/// 7. Assertions — any unmet => `Fail`
/// 8. Otherwise => `Pass`
pub fn grade_scenario(
    root: &Path,
    manifest: &ScenarioManifest,
    provider: &dyn EvidenceProvider,
) -> SpvVerdict {
    if verify_manifest(manifest).is_err() {
        return SpvVerdict::GateUntrusted;
    }
    if apply_policy(&manifest.payload.policy, &default_floors()).is_err() {
        return SpvVerdict::GateUntrusted;
    }

    let rel_paths: Vec<String> = manifest
        .payload
        .destinations
        .iter()
        .map(destination_rel)
        .collect();
    let rel_refs: Vec<&str> = rel_paths.iter().map(String::as_str).collect();
    let snapshot = match snapshot_tree(root, &rel_refs) {
        Ok(s) => s,
        Err(_) => return SpvVerdict::Incomplete,
    };

    for dest in &manifest.payload.destinations {
        let rel = destination_rel(dest);
        match snapshot.files.iter().find(|(p, _)| *p == rel) {
            Some((_, hash)) if *hash == dest.content_hash => {}
            _ => return SpvVerdict::Incomplete,
        }
    }

    let covered = provider.covered_destinations();
    for rel in &rel_paths {
        if !covered.iter().any(|c| c == rel) {
            return SpvVerdict::Incomplete;
        }
    }

    if !provider.is_fresh() {
        return SpvVerdict::Incomplete;
    }

    let mut file_paths: Vec<String> = rel_paths;
    for assertion in manifest
        .payload
        .positive_assertions
        .iter()
        .chain(manifest.payload.negative_assertions.iter())
    {
        let file = match &assertion.kind {
            AssertionKind::FileContains { file, .. }
            | AssertionKind::FindingAbsent { file, .. }
            | AssertionKind::SymbolPresent { file, .. } => file.clone(),
        };
        if !file_paths.iter().any(|p| p == &file) {
            file_paths.push(file);
        }
    }
    let files: Vec<(String, String)> = file_paths
        .iter()
        .filter_map(|p| provider.file_content(p).map(|c| (p.clone(), c)))
        .collect();
    let findings = provider.fresh_findings();

    for assertion in manifest
        .payload
        .positive_assertions
        .iter()
        .chain(manifest.payload.negative_assertions.iter())
    {
        if !assertion.check(&files, findings) {
            return SpvVerdict::Fail;
        }
    }

    SpvVerdict::Pass
}

/// Inputs gathered from phase-1/2 primitive verifiers for the independent decision.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerdictInputs {
    manifest_ok: bool,
    policy_ok: bool,
    snapshot_ok: bool,
    hash_match: bool,
    coverage_ok: bool,
    fresh: bool,
    assertions_ok: bool,
}

/// Explicit truth-table decision for the independent verdict path.
/// First matching arm wins. Precedence: GateUntrusted > Fail > Incomplete > Pass
/// (Fail is reached only after GateUntrusted and Incomplete conditions are clear).
fn decide_verdict(inputs: &VerdictInputs) -> SpvVerdict {
    match (
        inputs.manifest_ok,
        inputs.policy_ok,
        inputs.snapshot_ok,
        inputs.hash_match,
        inputs.coverage_ok,
        inputs.fresh,
        inputs.assertions_ok,
    ) {
        (false, _, _, _, _, _, _) => SpvVerdict::GateUntrusted,
        (true, false, _, _, _, _, _) => SpvVerdict::GateUntrusted,
        (true, true, false, _, _, _, _) => SpvVerdict::Incomplete,
        (true, true, true, false, _, _, _) => SpvVerdict::Incomplete,
        (true, true, true, true, false, _, _) => SpvVerdict::Incomplete,
        (true, true, true, true, true, false, _) => SpvVerdict::Incomplete,
        (true, true, true, true, true, true, false) => SpvVerdict::Fail,
        (true, true, true, true, true, true, true) => SpvVerdict::Pass,
    }
}

fn collect_assertion_files(manifest: &ScenarioManifest, rel_paths: &[String]) -> Vec<String> {
    let mut file_paths: Vec<String> = rel_paths.to_vec();
    for assertion in manifest
        .payload
        .positive_assertions
        .iter()
        .chain(manifest.payload.negative_assertions.iter())
    {
        let file = match &assertion.kind {
            AssertionKind::FileContains { file, .. }
            | AssertionKind::FindingAbsent { file, .. }
            | AssertionKind::SymbolPresent { file, .. } => file.clone(),
        };
        if !file_paths.iter().any(|p| p == &file) {
            file_paths.push(file);
        }
    }
    file_paths
}

fn compute_verdict_inputs(
    root: &Path,
    manifest: &ScenarioManifest,
    provider: &dyn EvidenceProvider,
) -> VerdictInputs {
    let manifest_ok = verify_manifest(manifest).is_ok();
    let policy_ok = apply_policy(&manifest.payload.policy, &default_floors()).is_ok();

    let rel_paths: Vec<String> = manifest
        .payload
        .destinations
        .iter()
        .map(destination_rel)
        .collect();
    let rel_refs: Vec<&str> = rel_paths.iter().map(String::as_str).collect();
    let snapshot = snapshot_tree(root, &rel_refs);
    let snapshot_ok = snapshot.is_ok();
    let hash_match = match &snapshot {
        Ok(snap) => manifest.payload.destinations.iter().all(|dest| {
            let rel = destination_rel(dest);
            snap.files
                .iter()
                .find(|(p, _)| *p == rel)
                .map(|(_, hash)| *hash == dest.content_hash)
                .unwrap_or(false)
        }),
        Err(_) => false,
    };

    let covered = provider.covered_destinations();
    let coverage_ok = rel_paths.iter().all(|rel| covered.iter().any(|c| c == rel));
    let fresh = provider.is_fresh();

    let file_paths = collect_assertion_files(manifest, &rel_paths);
    let files: Vec<(String, String)> = file_paths
        .iter()
        .filter_map(|p| provider.file_content(p).map(|c| (p.clone(), c)))
        .collect();
    let findings = provider.fresh_findings();
    let assertions_ok = manifest
        .payload
        .positive_assertions
        .iter()
        .chain(manifest.payload.negative_assertions.iter())
        .all(|a| a.check(&files, findings));

    VerdictInputs {
        manifest_ok,
        policy_ok,
        snapshot_ok,
        hash_match,
        coverage_ok,
        fresh,
        assertions_ok,
    }
}

/// Independent verdict decision (I9).
///
/// Independence rule: this MUST NOT call [`grade_scenario`]. It reuses the same
/// primitive verifiers (manifest, policy, snapshot, coverage, freshness,
/// assertions) but decides via a separate explicit truth table so a bug in one
/// decision path is unlikely to be mirrored in the other.
/// Precedence implemented: GateUntrusted > Fail > Incomplete > Pass
/// (table order: GateUntrusted, then Incomplete conditions, then Fail, then Pass).
pub fn grade_scenario_independent(
    root: &Path,
    manifest: &ScenarioManifest,
    provider: &dyn EvidenceProvider,
) -> SpvVerdict {
    let inputs = compute_verdict_inputs(root, manifest, provider);
    decide_verdict(&inputs)
}

/// Kind of self-test mutation applied to a scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationKind {
    FlipPositiveAssertion,
    MoveDestination,
    WeakenThreshold,
    InjectBadFinding,
    CanaryImpossible,
    VacuousTest,
}

/// A mutated (or canary) scenario derived from an operator-signed manifest.
#[derive(Debug, Clone)]
pub struct MutatedScenario {
    pub name: String,
    pub kind: MutationKind,
    pub manifest: ScenarioManifest,
    pub injected_finding: Option<Finding>,
    pub broken_files: Vec<(String, String)>,
}

fn first_finding_absent(assertions: &[Assertion]) -> Option<&Assertion> {
    assertions
        .iter()
        .find(|a| matches!(a.kind, AssertionKind::FindingAbsent { .. }))
}

/// Derive the self-test mutation set. Re-signs mutated payloads with `signing_key`
/// so mutations (a)–(d) and (f) exercise verdict logic rather than signature failure.
/// Canaries (e) are also correctly signed; the catch must come from assertion logic.
///
/// `FlipPositiveAssertion` is omitted (skipped by the runner) when no
/// `FileContains` positive assertion exists to flip.
pub fn derive_mutations(
    manifest: &ScenarioManifest,
    signing_key: &SigningKey,
) -> Result<Vec<MutatedScenario>> {
    let mut out = Vec::new();

    // (a) FlipPositiveAssertion — flip first FileContains among positives, else skip.
    // If the first positive is not FileContains, flip the first FileContains in
    // positive_assertions; if none exists, omit (runner records skipped).
    if let Some(idx) = manifest
        .payload
        .positive_assertions
        .iter()
        .position(|a| matches!(a.kind, AssertionKind::FileContains { .. }))
    {
        let mut payload = manifest.payload.clone();
        if let AssertionKind::FileContains { needle, .. } =
            &mut payload.positive_assertions[idx].kind
        {
            needle.push_str("__SPV_MUTATED_ABSENT__");
        }
        let signed = sign_manifest(&payload, signing_key)?;
        out.push(MutatedScenario {
            name: "flip-positive-assertion".into(),
            kind: MutationKind::FlipPositiveAssertion,
            manifest: signed,
            injected_finding: None,
            broken_files: vec![],
        });
    }

    // (b) MoveDestination — first destination path + ".mutated" (must not exist).
    {
        let mut payload = manifest.payload.clone();
        if let Some(dest) = payload.destinations.first_mut() {
            let moved = format!("{}.mutated", destination_rel(dest));
            dest.canonical_path = PathBuf::from(&moved);
            let signed = sign_manifest(&payload, signing_key)?;
            out.push(MutatedScenario {
                name: "move-destination".into(),
                kind: MutationKind::MoveDestination,
                manifest: signed,
                injected_finding: None,
                broken_files: vec![],
            });
        }
    }

    // (c) WeakenThreshold — diagnostics below floor => GateUntrusted via apply_policy.
    {
        let mut payload = manifest.payload.clone();
        payload.policy.diagnostics = Level::Low;
        let signed = sign_manifest(&payload, signing_key)?;
        out.push(MutatedScenario {
            name: "weaken-threshold".into(),
            kind: MutationKind::WeakenThreshold,
            manifest: signed,
            injected_finding: None,
            broken_files: vec![],
        });
    }

    // (d) InjectBadFinding — inject a finding that violates a FindingAbsent assertion.
    {
        let mut injected_finding: Option<Finding> = None;
        let mut payload = manifest.payload.clone();
        let mut need_resign = false;

        if let Some(a) = first_finding_absent(&manifest.payload.negative_assertions) {
            if let AssertionKind::FindingAbsent { rule, file } = &a.kind {
                injected_finding = Some(Finding::new(
                    "spv",
                    rule,
                    file,
                    1,
                    0.95,
                    "spv-inject",
                    "injected bad finding".into(),
                ));
            }
        } else if let Some(a) = first_finding_absent(&manifest.payload.positive_assertions) {
            if let AssertionKind::FindingAbsent { rule, file } = &a.kind {
                injected_finding = Some(Finding::new(
                    "spv",
                    rule,
                    file,
                    1,
                    0.95,
                    "spv-inject",
                    "injected bad finding".into(),
                ));
            }
        } else if let Some(dest) = manifest.payload.destinations.first() {
            let file = destination_rel(dest);
            let rule = "spv-injected".to_string();
            payload.negative_assertions.push(Assertion {
                id: "spv-injected-neg".into(),
                kind: AssertionKind::FindingAbsent {
                    rule: rule.clone(),
                    file: file.clone(),
                },
            });
            injected_finding = Some(Finding::new(
                "spv",
                &rule,
                &file,
                1,
                0.95,
                "spv-inject",
                "injected bad finding".into(),
            ));
            need_resign = true;
        }

        let signed = if need_resign {
            sign_manifest(&payload, signing_key)?
        } else {
            manifest.clone()
        };
        out.push(MutatedScenario {
            name: "inject-bad-finding".into(),
            kind: MutationKind::InjectBadFinding,
            manifest: signed,
            injected_finding,
            broken_files: vec![],
        });
    }

    // (e) CanaryImpossible — two correctly signed but logically impossible manifests.
    {
        let first_file = manifest
            .payload
            .destinations
            .first()
            .map(destination_rel)
            .unwrap_or_else(|| "missing".into());

        let mut payload1 = manifest.payload.clone();
        payload1.id = format!("{}/canary-1", manifest.payload.id);
        payload1.positive_assertions = vec![Assertion {
            id: "canary-1-pos".into(),
            kind: AssertionKind::FileContains {
                file: first_file.clone(),
                needle: "__SPV_IMPOSSIBLE_NEEDLE__".into(),
            },
        }];
        // Keep negatives empty so only the impossible positive drives Fail.
        payload1.negative_assertions = vec![];
        let signed1 = sign_manifest(&payload1, signing_key)?;
        out.push(MutatedScenario {
            name: "canary-1".into(),
            kind: MutationKind::CanaryImpossible,
            manifest: signed1,
            injected_finding: None,
            broken_files: vec![],
        });

        let mut payload2 = manifest.payload.clone();
        payload2.id = format!("{}/canary-2", manifest.payload.id);
        payload2.positive_assertions = vec![Assertion {
            id: "canary-2-pos".into(),
            kind: AssertionKind::SymbolPresent {
                file: first_file,
                symbol: "__SPV_IMPOSSIBLE_SYMBOL__".into(),
            },
        }];
        payload2.negative_assertions = vec![];
        let signed2 = sign_manifest(&payload2, signing_key)?;
        out.push(MutatedScenario {
            name: "canary-2".into(),
            kind: MutationKind::CanaryImpossible,
            manifest: signed2,
            injected_finding: None,
            broken_files: vec![],
        });
    }

    // (f) VacuousTest — identical manifest; runner grades with broken file contents.
    {
        let mut broken_files = Vec::new();
        if let Some(a) = manifest.payload.positive_assertions.first() {
            match &a.kind {
                AssertionKind::FileContains { file, needle: _ } => {
                    // Content with the needle removed (empty is certainly without it).
                    broken_files.push((file.clone(), String::new()));
                }
                AssertionKind::SymbolPresent { file, .. }
                | AssertionKind::FindingAbsent { file, .. } => {
                    broken_files.push((file.clone(), String::new()));
                }
            }
        } else if let Some(dest) = manifest.payload.destinations.first() {
            broken_files.push((destination_rel(dest), String::new()));
        }
        out.push(MutatedScenario {
            name: "vacuous-test".into(),
            kind: MutationKind::VacuousTest,
            manifest: manifest.clone(),
            injected_finding: None,
            broken_files,
        });
    }

    Ok(out)
}

/// Outcome of grading one mutation (or skipped derivation) in one self-test run.
#[derive(Debug, Clone)]
pub struct MutationOutcome {
    pub name: String,
    pub kind: MutationKind,
    pub verdict: SpvVerdict,
    pub caught: bool,
    pub skipped: bool,
}

/// Aggregate self-test report. v1 bar is 100% catch rate; anything less is untrusted.
#[derive(Debug, Clone)]
pub struct SelfTestReport {
    pub mutations: Vec<MutationOutcome>,
    pub canary_caught: usize,
    pub canary_total: usize,
    pub score: f32,
    pub passed: bool,
}

/// Provider overlay used by the self-test runner: appends an injected finding and
/// overrides file contents for vacuous-test broken paths.
struct OverlayProvider<'a> {
    base: &'a dyn EvidenceProvider,
    findings: Vec<Finding>,
    broken: HashMap<String, String>,
}

impl EvidenceProvider for OverlayProvider<'_> {
    fn fresh_findings(&self) -> &[Finding] {
        &self.findings
    }

    fn file_content(&self, rel_path: &str) -> Option<String> {
        if let Some(c) = self.broken.get(rel_path) {
            return Some(c.clone());
        }
        self.base.file_content(rel_path)
    }

    fn covered_destinations(&self) -> &[String] {
        self.base.covered_destinations()
    }

    fn is_fresh(&self) -> bool {
        self.base.is_fresh()
    }

    fn tool_attribution(&self, finding: &Finding) -> Option<(String, String, String, String)> {
        self.base.tool_attribution(finding)
    }
}

fn grade_mutated(
    root: &Path,
    mutation: &MutatedScenario,
    provider: &dyn EvidenceProvider,
) -> SpvVerdict {
    let mut findings = provider.fresh_findings().to_vec();
    if let Some(f) = &mutation.injected_finding {
        findings.push(f.clone());
    }
    let broken: HashMap<String, String> = mutation.broken_files.iter().cloned().collect();
    let overlay = OverlayProvider {
        base: provider,
        findings,
        broken,
    };
    grade_scenario(root, &mutation.manifest, &overlay)
}

/// Run the self-test mutation suite `runs` times (caller passes 2 in v1).
///
/// `score` = fraction of non-skipped mutations caught in *every* run.
/// `passed` requires score == 1.0, all canaries caught every run, at least one
/// non-skipped mutation, and at least one canary. v1 bar is 100% — anything less
/// means the gate is weak and must yield `GateUntrusted` (phase 3 enforces that).
pub fn run_self_test(
    root: &Path,
    manifest: &ScenarioManifest,
    provider: &dyn EvidenceProvider,
    signing_key: &SigningKey,
    runs: u8,
) -> Result<SelfTestReport> {
    if runs < 1 {
        bail!("runs must be >= 1");
    }

    let derived = derive_mutations(manifest, signing_key)?;
    let has_flip = derived
        .iter()
        .any(|m| m.kind == MutationKind::FlipPositiveAssertion);

    let mut mutations: Vec<MutationOutcome> = Vec::new();

    for _ in 0..runs {
        if !has_flip {
            mutations.push(MutationOutcome {
                name: "flip-positive-assertion".into(),
                kind: MutationKind::FlipPositiveAssertion,
                verdict: SpvVerdict::Incomplete,
                caught: false,
                skipped: true,
            });
        }
        for m in &derived {
            let verdict = grade_mutated(root, m, provider);
            let caught = verdict != SpvVerdict::Pass;
            mutations.push(MutationOutcome {
                name: m.name.clone(),
                kind: m.kind.clone(),
                verdict,
                caught,
                skipped: false,
            });
        }
    }

    // Aggregate by name: must be caught in every non-skipped run outcome.
    let mut by_name: HashMap<String, Vec<&MutationOutcome>> = HashMap::new();
    for o in &mutations {
        by_name.entry(o.name.clone()).or_default().push(o);
    }

    let mut caught_count = 0usize;
    let mut scored_total = 0usize;
    let mut canary_caught = 0usize;
    let mut canary_total = 0usize;

    for outcomes in by_name.values() {
        let skipped = outcomes.iter().all(|o| o.skipped);
        let is_canary = outcomes
            .first()
            .map(|o| o.kind == MutationKind::CanaryImpossible)
            .unwrap_or(false);

        if skipped {
            continue;
        }
        let caught_every = outcomes.iter().all(|o| o.caught);
        if is_canary {
            canary_total += 1;
            if caught_every {
                canary_caught += 1;
            }
        } else {
            scored_total += 1;
            if caught_every {
                caught_count += 1;
            }
        }
    }

    let score = if scored_total == 0 {
        0.0
    } else {
        caught_count as f32 / scored_total as f32
    };

    // v1 mutation bar is 100% — anything less means the gate is weak.
    // `scored_total` is (total - skipped) for non-canary mutations; canaries are
    // checked separately and must also be fully caught.
    let passed =
        score == 1.0 && canary_caught == canary_total && scored_total >= 1 && canary_total >= 1;

    Ok(SelfTestReport {
        mutations,
        canary_caught,
        canary_total,
        score,
        passed,
    })
}

/// Test-only evidence provider used by phase-2 self-test and grading tests.
#[cfg(test)]
pub(crate) struct FakeEvidence {
    pub findings: Vec<Finding>,
    pub files: HashMap<String, String>,
    pub covered: Vec<String>,
    pub fresh: bool,
}

#[cfg(test)]
impl EvidenceProvider for FakeEvidence {
    fn fresh_findings(&self) -> &[Finding] {
        &self.findings
    }

    fn file_content(&self, rel_path: &str) -> Option<String> {
        self.files.get(rel_path).cloned()
    }

    fn covered_destinations(&self) -> &[String] {
        &self.covered
    }

    fn is_fresh(&self) -> bool {
        self.fresh
    }

    fn tool_attribution(&self, _finding: &Finding) -> Option<(String, String, String, String)> {
        Some(("fake-tool".into(), "0.1.0".into(), "aa".into(), "bb".into()))
    }
}

/// Self-test repetitions for [`run_spv`]. v1 default is 2.
#[derive(Debug, Clone)]
pub struct SpvRunConfig {
    pub runs: u8,
}

impl Default for SpvRunConfig {
    fn default() -> Self {
        Self { runs: 2 }
    }
}

/// Format current UTC time as `YYYY-MM-DDTHH:MM:SSZ` using [`SystemTime`].
/// No extra time crate; sub-second precision is truncated. Civil-date conversion
/// uses the standard days-from-civil algorithm (Howard Hinnant).
fn utc_now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_utc_ymd_hms(secs)
}

fn civil_utc_ymd_hms(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;

    // civil_from_days (Howard Hinnant), Unix epoch = 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Sha256 of the current executable's bytes. If the path is unreadable, returns
/// the literal string `"unknown"` (not a hash of that string).
fn current_gate_binary_hash() -> String {
    match std::env::current_exe().and_then(std::fs::read) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => "unknown".into(),
    }
}

fn policy_hash(policy: &Policy) -> String {
    match serde_json::to_vec(policy) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex(&[]),
    }
}

fn canary_results_from_report(report: &SelfTestReport) -> Vec<CanaryResult> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for m in &report.mutations {
        if m.kind != MutationKind::CanaryImpossible {
            continue;
        }
        if seen.iter().any(|n| n == &m.name) {
            continue;
        }
        seen.push(m.name.clone());
        let caught_every = report
            .mutations
            .iter()
            .filter(|o| o.name == m.name)
            .all(|o| o.caught);
        if caught_every {
            out.push(CanaryResult::Caught {
                name: m.name.clone(),
            });
        } else {
            out.push(CanaryResult::Missed {
                name: m.name.clone(),
            });
        }
    }
    out
}

fn coverage_all_uncovered(manifest: &ScenarioManifest) -> CoverageSet {
    CoverageSet {
        covered: vec![],
        uncovered: manifest
            .payload
            .destinations
            .iter()
            .map(destination_rel)
            .collect(),
    }
}

fn coverage_from_provider(
    manifest: &ScenarioManifest,
    provider: &dyn EvidenceProvider,
) -> CoverageSet {
    let covered_src = provider.covered_destinations();
    let mut covered = Vec::new();
    let mut uncovered = Vec::new();
    for dest in &manifest.payload.destinations {
        let rel = destination_rel(dest);
        if covered_src.iter().any(|c| c == &rel) {
            covered.push(rel);
        } else {
            uncovered.push(rel);
        }
    }
    CoverageSet { covered, uncovered }
}

fn build_provenance(
    provider: &dyn EvidenceProvider,
    snapshot: &Option<TreeSnapshot>,
    tree_id: &str,
    executed_at: &str,
) -> Vec<ProvenanceTuple> {
    let file_hashes: HashMap<String, String> = snapshot
        .as_ref()
        .map(|s| s.files.iter().cloned().collect())
        .unwrap_or_default();
    provider
        .fresh_findings()
        .iter()
        .map(|f| {
            let file_key = f.file.replace('\\', "/");
            let file_content_hash = file_hashes.get(&file_key).cloned().unwrap_or_default();
            let (tool_id, tool_version, tool_path_hash, args_hash) =
                provider.tool_attribution(f).unwrap_or_default();
            ProvenanceTuple {
                finding_fingerprint_full: full_fingerprint(f),
                file_content_hash,
                tool_id,
                tool_version,
                tool_path_hash,
                args_hash,
                tree_id: tree_id.to_string(),
                executed_at: executed_at.to_string(),
            }
        })
        .collect()
}

/// Parameters for [`unsigned_receipt_shell`], bundled so the helper stays
/// under clippy's argument-count limit.
struct ReceiptShellArgs {
    verdict: SpvVerdict,
    tree_hash: String,
    coverage: CoverageSet,
    mutation_score: f32,
    canary_results: Vec<CanaryResult>,
    findings: Vec<ProvenanceTuple>,
    verdict_disagreement: bool,
}

fn unsigned_receipt_shell(manifest: &ScenarioManifest, args: ReceiptShellArgs) -> VerdictReceipt {
    VerdictReceipt {
        verdict: args.verdict,
        manifest_hash: manifest_hash(manifest),
        tree_hash: args.tree_hash,
        coverage: args.coverage,
        mutation_score: args.mutation_score,
        canary_results: args.canary_results,
        findings: args.findings,
        policy_hash: policy_hash(&manifest.payload.policy),
        gate_binary_hash: current_gate_binary_hash(),
        verdict_disagreement: args.verdict_disagreement,
        signature_hex: String::new(),
        public_key_hex: String::new(),
    }
}

/// Run the full SPV pipeline: self-test (I1), dual verdict (I9), tree binding (I8),
/// provenance (I2), and signed receipt emission.
///
/// Never panics on bad input — failures become verdicts on the returned receipt.
///
/// TODO(SPV-I4): baseline entries may only drop as fixed on a fresh read of their
/// destination; v1 never drops baselines, so I4 is unimplemented, not satisfied.
pub fn run_spv(
    root: &Path,
    manifest: &ScenarioManifest,
    provider: &dyn EvidenceProvider,
    signing_key: &SigningKey,
    config: &SpvRunConfig,
) -> VerdictReceipt {
    // TODO(SPV-I4): baseline entries may only drop as fixed on a fresh read of
    // their destination; v1 never drops baselines, so I4 is unimplemented, not
    // satisfied. This function does not read or write baselines.

    let self_test = run_self_test(root, manifest, provider, signing_key, config.runs);
    let (self_passed, mutation_score, canary_results) = match &self_test {
        Ok(report) => (
            report.passed,
            report.score,
            canary_results_from_report(report),
        ),
        Err(_) => (false, 0.0, Vec::new()),
    };

    if !self_passed {
        // I1: do not grade anything when self-test fails.
        let mut receipt = unsigned_receipt_shell(
            manifest,
            ReceiptShellArgs {
                verdict: SpvVerdict::GateUntrusted,
                tree_hash: String::new(),
                coverage: coverage_all_uncovered(manifest),
                mutation_score,
                canary_results,
                findings: Vec::new(),
                verdict_disagreement: false,
            },
        );
        let _ = sign_receipt(&mut receipt, signing_key);
        return receipt;
    }

    // Snapshot at grading time (I8 binding baseline).
    let rel_paths: Vec<String> = manifest
        .payload
        .destinations
        .iter()
        .map(destination_rel)
        .collect();
    let rel_refs: Vec<&str> = rel_paths.iter().map(String::as_str).collect();
    let grading_snap = snapshot_tree(root, &rel_refs).ok();
    let grading_hash = grading_snap
        .as_ref()
        .map(|s| s.tree_hash.clone())
        .unwrap_or_default();

    // Dual verdict (I9).
    let primary = grade_scenario(root, manifest, provider);
    let independent = grade_scenario_independent(root, manifest, provider);
    let (mut verdict, verdict_disagreement) = if primary != independent {
        (SpvVerdict::Incomplete, true)
    } else {
        (primary, false)
    };

    // Receipt tree binding (I8): re-snapshot; mid-run change => Incomplete.
    if let Ok(re_snap) = snapshot_tree(root, &rel_refs) {
        if grading_snap
            .as_ref()
            .map(|s| s.tree_hash != re_snap.tree_hash)
            .unwrap_or(true)
        {
            verdict = SpvVerdict::Incomplete;
        }
    } else if grading_snap.is_some() {
        // Tree became unreadable after grading.
        verdict = SpvVerdict::Incomplete;
    }

    let executed_at = utc_now_rfc3339();
    let findings = build_provenance(provider, &grading_snap, &grading_hash, &executed_at);
    let coverage = coverage_from_provider(manifest, provider);

    let mut receipt = unsigned_receipt_shell(
        manifest,
        ReceiptShellArgs {
            verdict,
            tree_hash: grading_hash,
            coverage,
            mutation_score,
            canary_results,
            findings,
            verdict_disagreement,
        },
    );
    let _ = sign_receipt(&mut receipt, signing_key);
    receipt
}

/// Consumer-side I8 check: re-snapshot `rel_paths` and compare to `receipt.tree_hash`.
/// Returns Ok(true) if equal, Ok(false) if different, Err on snapshot failure.
pub fn verify_receipt_tree(
    root: &Path,
    receipt: &VerdictReceipt,
    rel_paths: &[&str],
) -> Result<bool> {
    let snap = snapshot_tree(root, rel_paths)?;
    Ok(snap.tree_hash == receipt.tree_hash)
}

/// TODO(SPV-I4): baseline entries may only drop as fixed on a fresh read of their
/// destination. Stub panics until I4 is implemented; v1 never calls it —
/// baselines are left untouched (known v1 limitation, see module docs).
#[allow(dead_code)]
fn baseline_entry_dropped_as_fixed(
    _baseline_path: &Path,
    _finding_fingerprint: &str,
    _fresh_findings: &[Finding],
) -> bool {
    todo!("TODO(SPV-I4): drop baseline entries as fixed only on a fresh read of their destination")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    fn test_key_a() -> SigningKey {
        SigningKey::from_bytes(&[1u8; 32])
    }

    /// Build a valid signed manifest over a tempdir tree: one destination with
    /// known content, one positive FileContains, one negative FindingAbsent,
    /// policy = defaults.
    fn fake_manifest(dir: &Path) -> (ScenarioManifest, SigningKey) {
        let rel = "src/code.py";
        let content = "def hello():\n    return 42\n";
        fs::create_dir_all(dir.join("src")).expect("mkdir");
        fs::write(dir.join(rel), content).expect("write");
        let hash = sha256_hex(content.as_bytes());
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let payload = ManifestPayload {
            id: "test-scenario".into(),
            version: 1,
            destinations: vec![Destination {
                canonical_path: PathBuf::from(rel),
                content_hash: hash,
            }],
            positive_assertions: vec![Assertion {
                id: "pos-1".into(),
                kind: AssertionKind::FileContains {
                    file: rel.into(),
                    needle: "hello".into(),
                },
            }],
            negative_assertions: vec![Assertion {
                id: "neg-1".into(),
                kind: AssertionKind::FindingAbsent {
                    rule: "banned-rule".into(),
                    file: rel.into(),
                },
            }],
            tools: vec![],
            policy: Policy {
                enforce_dead_code: false,
                dependencies: Level::Medium,
                weakening: Level::Medium,
                diagnostics: Level::Medium,
                rules: Level::Medium,
            },
        };
        let manifest = sign_manifest(&payload, &key).expect("sign");
        (manifest, key)
    }

    fn matching_evidence(content: &str) -> FakeEvidence {
        let rel = "src/code.py";
        let mut files = HashMap::new();
        files.insert(rel.into(), content.into());
        FakeEvidence {
            findings: vec![],
            files,
            covered: vec![rel.into()],
            fresh: true,
        }
    }

    const FAKE_CONTENT: &str = "def hello():\n    return 42\n";

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
            verdict_disagreement: false,
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

    // --- Phase 2: grade_scenario + self-test ---

    #[test]
    fn grade_passes_on_valid_scenario() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::Pass);
    }

    #[test]
    fn grade_gate_untrusted_on_tampered_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut manifest, _key) = fake_manifest(dir.path());
        manifest.payload.destinations[0].content_hash = "deadbeef".into();
        let evidence = matching_evidence(FAKE_CONTENT);
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::GateUntrusted);
    }

    #[test]
    fn grade_gate_untrusted_on_policy_below_floor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let mut payload = manifest.payload.clone();
        payload.policy.diagnostics = Level::Low;
        let signed = sign_manifest(&payload, &key).expect("sign");
        let evidence = matching_evidence(FAKE_CONTENT);
        let v = grade_scenario(dir.path(), &signed, &evidence);
        assert_eq!(v, SpvVerdict::GateUntrusted);
    }

    #[test]
    fn grade_incomplete_on_uncovered_destination() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());
        let mut evidence = matching_evidence(FAKE_CONTENT);
        evidence.covered.clear();
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::Incomplete);
    }

    #[test]
    fn grade_incomplete_on_stale_manifest_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());
        fs::write(dir.path().join("src/code.py"), b"changed").expect("change");
        let evidence = matching_evidence(FAKE_CONTENT);
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::Incomplete);
    }

    #[test]
    fn grade_incomplete_on_non_fresh_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());
        let mut evidence = matching_evidence(FAKE_CONTENT);
        evidence.fresh = false;
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::Incomplete);
    }

    #[test]
    fn grade_fail_on_broken_positive_assertion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());
        let mut evidence = matching_evidence(FAKE_CONTENT);
        evidence
            .files
            .insert("src/code.py".into(), "no needle here".into());
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::Fail);
    }

    #[test]
    fn grade_fail_on_negative_assertion_violation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());
        let mut evidence = matching_evidence(FAKE_CONTENT);
        evidence.findings.push(Finding::new(
            "spv",
            "banned-rule",
            "src/code.py",
            1,
            0.95,
            "t",
            "present".into(),
        ));
        let v = grade_scenario(dir.path(), &manifest, &evidence);
        assert_eq!(v, SpvVerdict::Fail);
    }

    #[test]
    fn self_test_catches_flip_positive_assertion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 1).expect("self-test");
        let flip = report
            .mutations
            .iter()
            .find(|o| o.kind == MutationKind::FlipPositiveAssertion)
            .expect("flip outcome");
        assert!(flip.caught);
        assert!(!flip.skipped);
    }

    #[test]
    fn self_test_catches_move_destination() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 1).expect("self-test");
        let m = report
            .mutations
            .iter()
            .find(|o| o.kind == MutationKind::MoveDestination)
            .expect("move outcome");
        assert!(m.caught);
    }

    #[test]
    fn self_test_catches_weaken_threshold() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 1).expect("self-test");
        let m = report
            .mutations
            .iter()
            .find(|o| o.kind == MutationKind::WeakenThreshold)
            .expect("weaken outcome");
        assert!(m.caught);
        assert_eq!(m.verdict, SpvVerdict::GateUntrusted);
    }

    #[test]
    fn self_test_catches_inject_bad_finding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 1).expect("self-test");
        let m = report
            .mutations
            .iter()
            .find(|o| o.kind == MutationKind::InjectBadFinding)
            .expect("inject outcome");
        assert!(m.caught);
    }

    #[test]
    fn self_test_canaries_100_percent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 2).expect("self-test");
        assert_eq!(report.canary_total, 2);
        assert_eq!(report.canary_caught, report.canary_total);
        for o in report
            .mutations
            .iter()
            .filter(|o| o.kind == MutationKind::CanaryImpossible)
        {
            assert!(o.caught);
            assert_ne!(o.verdict, SpvVerdict::Pass);
        }
    }

    #[test]
    fn self_test_catches_vacuous_test() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 1).expect("self-test");
        let m = report
            .mutations
            .iter()
            .find(|o| o.kind == MutationKind::VacuousTest)
            .expect("vacuous outcome");
        assert!(m.caught);
        assert_ne!(m.verdict, SpvVerdict::Pass);
    }

    /// Stateful provider: first `file_content` call returns content that includes
    /// the FlipPositiveAssertion mutated needle (so that mutation Passes / is missed
    /// once); later calls return normal content. Across runs=2 the flip is not
    /// caught every run => overall `passed == false`.
    struct FlakyEvidence {
        inner: FakeEvidence,
        calls: RefCell<u32>,
        // Full mutated needle (original needle + suffix). Appending only the
        // suffix never satisfies the flipped assertion: the mutated needle is
        // '<original>__SPV_MUTATED_ABSENT__', so the rig must return content
        // containing that whole string on the first call.
        mutated_needle: String,
    }

    impl EvidenceProvider for FlakyEvidence {
        fn fresh_findings(&self) -> &[Finding] {
            self.inner.fresh_findings()
        }

        fn file_content(&self, rel_path: &str) -> Option<String> {
            let mut n = self.calls.borrow_mut();
            *n += 1;
            let base = self.inner.file_content(rel_path)?;
            if *n == 1 {
                // Satisfy the mutated FileContains needle on the first grading only.
                Some(format!("{base}{}", self.mutated_needle))
            } else {
                Some(base)
            }
        }

        fn covered_destinations(&self) -> &[String] {
            self.inner.covered_destinations()
        }

        fn is_fresh(&self) -> bool {
            self.inner.is_fresh()
        }

        fn tool_attribution(&self, finding: &Finding) -> Option<(String, String, String, String)> {
            self.inner.tool_attribution(finding)
        }
    }

    #[test]
    fn self_test_twice_consecutive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 2).expect("self-test");
        assert!(report.passed);

        // The flip appends '__SPV_MUTATED_ABSENT__' to the original needle
        // (see derive_mutations); the rig must satisfy the FULL mutated needle.
        let needle = manifest
            .payload
            .positive_assertions
            .iter()
            .find_map(|a| match &a.kind {
                AssertionKind::FileContains { needle, .. } => Some(needle.clone()),
                _ => None,
            })
            .expect("fake manifest has a FileContains positive assertion");
        let flaky = FlakyEvidence {
            inner: matching_evidence(FAKE_CONTENT),
            calls: RefCell::new(0),
            mutated_needle: format!("{needle}__SPV_MUTATED_ABSENT__"),
        };
        let report2 = run_self_test(dir.path(), &manifest, &flaky, &key, 2).expect("self-test");
        assert!(!report2.passed);
    }

    #[test]
    fn self_test_fails_when_gate_is_blind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let blind = FakeEvidence {
            findings: vec![],
            files: {
                let mut m = HashMap::new();
                m.insert("src/code.py".into(), String::new());
                m
            },
            covered: vec!["src/code.py".into()],
            fresh: true,
        };
        let report = run_self_test(dir.path(), &manifest, &blind, &key, 2).expect("self-test");
        assert!(
            report.passed,
            "self-test must still catch mutations on a blind provider"
        );
        let grade = grade_scenario(dir.path(), &manifest, &blind);
        assert_eq!(
            grade,
            SpvVerdict::Fail,
            "blind gate must not Pass the real scenario"
        );
    }

    #[test]
    fn mutation_score_is_one_on_valid_scenario() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let report = run_self_test(dir.path(), &manifest, &evidence, &key, 2).expect("self-test");
        assert_eq!(report.score, 1.0);
        assert!(report.passed);
    }

    // --- Phase 3: run_spv, dual verdict, receipt emission ---

    /// Provider whose `file_content` always contains every needle, including the
    /// FlipPositiveAssertion mutated needle `__SPV_MUTATED_ABSENT__`. That makes
    /// the flip mutation grade Pass, so self-test fails.
    struct BlindFakeEvidence {
        inner: FakeEvidence,
    }

    impl EvidenceProvider for BlindFakeEvidence {
        fn fresh_findings(&self) -> &[Finding] {
            self.inner.fresh_findings()
        }

        fn file_content(&self, rel_path: &str) -> Option<String> {
            let base = self.inner.file_content(rel_path).unwrap_or_default();
            Some(format!("{base}hello__SPV_MUTATED_ABSENT__"))
        }

        fn covered_destinations(&self) -> &[String] {
            self.inner.covered_destinations()
        }

        fn is_fresh(&self) -> bool {
            self.inner.is_fresh()
        }

        fn tool_attribution(&self, finding: &Finding) -> Option<(String, String, String, String)> {
            self.inner.tool_attribution(finding)
        }
    }

    fn evidence_with_finding(content: &str) -> FakeEvidence {
        let mut evidence = matching_evidence(content);
        evidence.findings.push(Finding::new(
            "spv",
            "other-rule",
            "src/code.py",
            1,
            0.9,
            "fake",
            "benign finding".into(),
        ));
        evidence
    }

    #[test]
    fn run_spv_passes_valid_scenario() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = evidence_with_finding(FAKE_CONTENT);
        let receipt = run_spv(
            dir.path(),
            &manifest,
            &evidence,
            &key,
            &SpvRunConfig::default(),
        );
        assert_eq!(receipt.verdict, SpvVerdict::Pass);
        verify_receipt(&receipt).expect("verify");
        assert_eq!(receipt.mutation_score, 1.0);
        assert!(!receipt.canary_results.is_empty());
        assert!(receipt
            .canary_results
            .iter()
            .all(|c| matches!(c, CanaryResult::Caught { .. })));
        assert!(!receipt.verdict_disagreement);
        assert!(!receipt.findings.is_empty());
        for p in &receipt.findings {
            assert!(!p.tool_id.is_empty());
            assert!(!p.tool_version.is_empty());
            assert!(!p.tool_path_hash.is_empty());
            assert!(!p.args_hash.is_empty());
            assert!(!p.finding_fingerprint_full.is_empty());
            assert!(!p.tree_id.is_empty());
            assert!(!p.executed_at.is_empty());
        }
    }

    #[test]
    fn run_spv_gate_untrusted_when_self_test_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let report = run_self_test(
            dir.path(),
            &manifest,
            &matching_evidence(FAKE_CONTENT),
            &key,
            1,
        )
        .expect("baseline self-test passes with honest provider");
        assert!(report.passed);

        let blind = BlindFakeEvidence {
            inner: matching_evidence(FAKE_CONTENT),
        };
        let receipt = run_spv(
            dir.path(),
            &manifest,
            &blind,
            &key,
            &SpvRunConfig::default(),
        );
        assert_eq!(receipt.verdict, SpvVerdict::GateUntrusted);
        assert!(receipt.findings.is_empty());
        assert!(receipt.mutation_score < 1.0);
        assert!(receipt.coverage.covered.is_empty());
        assert!(!receipt.coverage.uncovered.is_empty());
    }

    #[test]
    fn dual_verdict_agrees_on_valid_scenario() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, _key) = fake_manifest(dir.path());

        // Valid => Pass
        let ok = matching_evidence(FAKE_CONTENT);
        assert_eq!(
            grade_scenario(dir.path(), &manifest, &ok),
            grade_scenario_independent(dir.path(), &manifest, &ok)
        );
        assert_eq!(grade_scenario(dir.path(), &manifest, &ok), SpvVerdict::Pass);

        // Failing positive assertion => Fail
        let mut failing = matching_evidence(FAKE_CONTENT);
        failing
            .files
            .insert("src/code.py".into(), "no needle".into());
        assert_eq!(
            grade_scenario(dir.path(), &manifest, &failing),
            grade_scenario_independent(dir.path(), &manifest, &failing)
        );
        assert_eq!(
            grade_scenario(dir.path(), &manifest, &failing),
            SpvVerdict::Fail
        );

        // Incomplete: non-fresh
        let mut incomplete = matching_evidence(FAKE_CONTENT);
        incomplete.fresh = false;
        assert_eq!(
            grade_scenario(dir.path(), &manifest, &incomplete),
            grade_scenario_independent(dir.path(), &manifest, &incomplete)
        );
        assert_eq!(
            grade_scenario(dir.path(), &manifest, &incomplete),
            SpvVerdict::Incomplete
        );
    }

    #[test]
    fn dual_verdict_disagreement_is_incomplete() {
        // The public dual-verdict disagreement path (receipt.verdict_disagreement
        // = true) cannot be triggered without a real divergence between
        // grade_scenario and grade_scenario_independent; by construction they
        // agree on every real input. Cover the decision table directly, and
        // cover the signed receipt field by constructing disagreement=true.
        let all_ok = VerdictInputs {
            manifest_ok: true,
            policy_ok: true,
            snapshot_ok: true,
            hash_match: true,
            coverage_ok: true,
            fresh: true,
            assertions_ok: true,
        };
        assert_eq!(decide_verdict(&all_ok), SpvVerdict::Pass);

        let mut fault = all_ok.clone();
        fault.manifest_ok = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::GateUntrusted);

        fault = all_ok.clone();
        fault.policy_ok = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::GateUntrusted);

        fault = all_ok.clone();
        fault.snapshot_ok = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::Incomplete);

        fault = all_ok.clone();
        fault.hash_match = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::Incomplete);

        fault = all_ok.clone();
        fault.coverage_ok = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::Incomplete);

        fault = all_ok.clone();
        fault.fresh = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::Incomplete);

        fault = all_ok.clone();
        fault.assertions_ok = false;
        assert_eq!(decide_verdict(&fault), SpvVerdict::Fail);

        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let receipt = run_spv(
            dir.path(),
            &manifest,
            &evidence,
            &key,
            &SpvRunConfig::default(),
        );
        assert!(!receipt.verdict_disagreement);

        let key = test_key_a();
        let mut disagreed = sample_receipt();
        disagreed.verdict_disagreement = true;
        disagreed.verdict = SpvVerdict::Incomplete;
        sign_receipt(&mut disagreed, &key).expect("sign");
        verify_receipt(&disagreed).expect("disagreement field is part of signed bytes");
    }

    #[test]
    fn receipt_tree_binding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let receipt = run_spv(
            dir.path(),
            &manifest,
            &evidence,
            &key,
            &SpvRunConfig::default(),
        );
        let rel = "src/code.py";
        assert!(verify_receipt_tree(dir.path(), &receipt, &[rel]).expect("tree ok"));
        fs::write(dir.path().join(rel), b"def hello():\n    return 43\n").expect("change");
        assert!(!verify_receipt_tree(dir.path(), &receipt, &[rel]).expect("tree changed"));
    }

    #[test]
    fn saved_only_never_counts_as_verified_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let mut evidence = matching_evidence(FAKE_CONTENT);
        evidence.fresh = false;
        evidence.findings.clear();
        let receipt = run_spv(
            dir.path(),
            &manifest,
            &evidence,
            &key,
            &SpvRunConfig::default(),
        );
        assert_eq!(receipt.verdict, SpvVerdict::Incomplete);
        assert_ne!(receipt.verdict, SpvVerdict::Pass);
    }

    #[test]
    fn uncovered_destination_is_incomplete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let mut evidence = matching_evidence(FAKE_CONTENT);
        evidence.covered.clear();
        let receipt = run_spv(
            dir.path(),
            &manifest,
            &evidence,
            &key,
            &SpvRunConfig::default(),
        );
        assert_eq!(receipt.verdict, SpvVerdict::Incomplete);
    }

    #[test]
    fn receipt_signature_verifies_and_tamper_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (manifest, key) = fake_manifest(dir.path());
        let evidence = matching_evidence(FAKE_CONTENT);
        let mut receipt = run_spv(
            dir.path(),
            &manifest,
            &evidence,
            &key,
            &SpvRunConfig::default(),
        );
        verify_receipt(&receipt).expect("verify");
        receipt.mutation_score = 0.5;
        assert!(verify_receipt(&receipt).is_err());
    }
}
