//! Effect transactions: state machine, operation keys, reconciliation and a
//! crash-injection conformance harness.
//!
//! PRD v11 references: section 7.6 (effect taxonomy & transactional semantics),
//! 8.5 (crash recovery & side-effect correctness), 8.6 (terminal states),
//! RS-1, Q16.
//!
//! Consequential effects are transactions, not just calls (PRD principle 11). The
//! runtime distinguishes planned, authorized, dispatched, acknowledged and
//! reconciled states; recovery never blindly repeats an effect whose commit state
//! is unknown (PRD 8.5). Idempotent effects may be retried with the same
//! operation key; non-idempotent effects with unknown commit enter reconciliation
//! rather than automatic retry.
#![forbid(unsafe_code)]

pub mod key;

use std::collections::HashMap;

use entelechy_ir::{EffectClass, EffectMetadata};
use serde::{Deserialize, Serialize};

pub use key::OperationKey;

/// The state of one effect in the transactional state machine (PRD 8.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectState {
    /// Intent recorded (write-ahead), not yet authorized.
    Planned,
    /// Authorized; a durable authorization record exists before dispatch.
    Authorized,
    /// Dispatched to the provider; acknowledgement not yet received.
    Dispatched,
    /// Provider acknowledged the effect.
    Acknowledged,
    /// Resolved by reconciliation after an unknown outcome.
    Reconciled,
}

/// Commit status of a consequential effect (PRD 7.6). Only `Unknown` enters
/// reconciliation (PRD 8.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitStatus {
    /// Known committed.
    Committed,
    /// Known not committed.
    NotCommitted,
    /// Unknown; must be reconciled before the run completes (PRD 8.6).
    Unknown,
}

/// A durable effect record (PRD 5.2 EffectRecord): append-only in spirit; each
/// transition rewrites the latest durable state under its operation key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectRecord {
    /// Operation key string.
    pub op_key: String,
    /// Capability invoked.
    pub capability: String,
    /// Hash of the canonical request (PRD 7.6).
    pub request_hash: String,
    /// Current state.
    pub state: EffectState,
    /// Current commit status.
    pub commit: CommitStatus,
    /// Whether the effect is idempotent (retry-safe with same key).
    pub idempotent: bool,
    /// Whether a read-back reconciliation query exists (Q16).
    pub read_back: bool,
    /// Whether the effect is irreversible (requires durable authz before dispatch).
    pub irreversible: bool,
}

impl EffectRecord {
    fn from_meta(op_key: &OperationKey, capability: &str, request_hash: &str, meta: &EffectMetadata) -> Self {
        Self {
            op_key: op_key.as_string(),
            capability: capability.to_string(),
            request_hash: request_hash.to_string(),
            state: EffectState::Planned,
            commit: CommitStatus::NotCommitted,
            idempotent: meta.idempotent,
            read_back: meta.read_back_supported,
            irreversible: meta.class == EffectClass::Irreversible,
        }
    }

    /// Whether this effect is eligible for automatic retry (PRD Q16): native
    /// idempotency or a read-back reconciliation query.
    pub fn auto_retry_eligible(&self) -> bool {
        self.idempotent || self.read_back
    }
}

/// A simulated external system that records commits per operation key. It dedups
/// idempotent effects by key, so a *silent duplicate* shows up as a commit count
/// greater than one for a single logical effect.
#[derive(Default)]
pub struct SimExternalSystem {
    commits: HashMap<String, u32>,
    raw_dispatches: u32,
}

impl SimExternalSystem {
    /// Create an empty system.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an effect. Idempotent effects dedup by key; non-idempotent effects
    /// commit on every apply (so a re-dispatch is a real duplicate).
    pub fn apply(&mut self, op_key: &str, idempotent: bool) {
        self.raw_dispatches += 1;
        let entry = self.commits.entry(op_key.to_string()).or_insert(0);
        if idempotent {
            *entry = 1;
        } else {
            *entry += 1;
        }
    }

    /// Read-back query: how many times this key committed (PRD 7.6, Q16).
    pub fn commit_count(&self, op_key: &str) -> u32 {
        self.commits.get(op_key).copied().unwrap_or(0)
    }

    /// Total raw dispatch calls (for observability).
    pub fn raw_dispatches(&self) -> u32 {
        self.raw_dispatches
    }
}

/// A durable write-ahead log keyed by operation key (PRD 7.6 write-ahead intent).
#[derive(Default)]
pub struct WriteAheadLog {
    records: HashMap<String, EffectRecord>,
}

impl WriteAheadLog {
    /// Empty log.
    pub fn new() -> Self {
        Self::default()
    }
    /// Durably record the latest state for an effect.
    pub fn put(&mut self, record: EffectRecord) {
        self.records.insert(record.op_key.clone(), record);
    }
    /// Fetch the durable record for a key.
    pub fn get(&self, op_key: &str) -> Option<&EffectRecord> {
        self.records.get(op_key)
    }
    /// All records (for recovery scans).
    pub fn records(&self) -> impl Iterator<Item = &EffectRecord> {
        self.records.values()
    }
}

/// Where to inject a simulated crash during an effect attempt (PRD 8.5: the
/// conformance suite injects crashes at every effect-state transition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    /// Crash before the intent is durably written (no durable trace).
    BeforeIntent,
    /// Crash after intent, before authorization.
    AfterIntent,
    /// Crash after authorization, before dispatch (no effect happened).
    AfterAuthorize,
    /// Crash after dispatch, before acknowledgement (commit is Unknown).
    AfterDispatch,
    /// Crash after acknowledgement (commit is Committed durably).
    AfterAck,
    /// No crash (the happy path).
    None,
}

/// The outcome of an effect attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Completed and acknowledged.
    Completed,
    /// Interrupted by a crash at the given point.
    Crashed(CrashPoint),
}

/// Drives consequential effects through the transactional state machine over a
/// durable WAL and an external system (PRD 8.5).
pub struct EffectManager {
    wal: WriteAheadLog,
    ext: SimExternalSystem,
}

impl EffectManager {
    /// Create a manager.
    pub fn new() -> Self {
        Self {
            wal: WriteAheadLog::new(),
            ext: SimExternalSystem::new(),
        }
    }

    /// Access the external system (e.g. to assert commit counts).
    pub fn external(&self) -> &SimExternalSystem {
        &self.ext
    }

    /// The durable log.
    pub fn wal(&self) -> &WriteAheadLog {
        &self.wal
    }

    /// Attempt a consequential effect, optionally crashing at `crash` (PRD 8.5).
    /// State transitions are persisted to the WAL before each externally visible
    /// step, so a crash leaves evidence to reconcile.
    pub fn attempt(
        &mut self,
        op_key: &OperationKey,
        capability: &str,
        request_hash: &str,
        meta: &EffectMetadata,
        crash: CrashPoint,
    ) -> AttemptOutcome {
        if crash == CrashPoint::BeforeIntent {
            // Power loss before anything durable: nothing to reconcile.
            return AttemptOutcome::Crashed(crash);
        }

        // 1. Write-ahead intent (durable before dispatch — PRD 7.6).
        let mut rec = EffectRecord::from_meta(op_key, capability, request_hash, meta);
        rec.state = EffectState::Planned;
        self.wal.put(rec.clone());
        if crash == CrashPoint::AfterIntent {
            return AttemptOutcome::Crashed(crash);
        }

        // 2. Authorize (durable authorization record before dispatch — PRD 8.5).
        rec.state = EffectState::Authorized;
        self.wal.put(rec.clone());
        if crash == CrashPoint::AfterAuthorize {
            return AttemptOutcome::Crashed(crash);
        }

        // 3. Dispatch: mark Dispatched + Unknown *before* the call, so a crash
        //    between dispatch and ack leaves an unknown-commit record (PRD 7.6).
        rec.state = EffectState::Dispatched;
        rec.commit = CommitStatus::Unknown;
        self.wal.put(rec.clone());
        self.ext.apply(&rec.op_key, rec.idempotent);
        if crash == CrashPoint::AfterDispatch {
            return AttemptOutcome::Crashed(crash);
        }

        // 4. Acknowledge.
        rec.state = EffectState::Acknowledged;
        rec.commit = CommitStatus::Committed;
        self.wal.put(rec);
        let _ = crash; // AfterAck / None fall through here.
        AttemptOutcome::Completed
    }

    /// Recover after a crash (PRD 8.5). Never blindly repeats an unknown-commit
    /// effect: idempotent / read-back effects are reconciled by read-back;
    /// others are left as reconciliation-required for human/compensation.
    ///
    /// Returns the number of effects left requiring human reconciliation.
    pub fn recover(&mut self) -> usize {
        let keys: Vec<String> = self.wal.records().map(|r| r.op_key.clone()).collect();
        let mut needs_human = 0;
        for k in keys {
            let mut rec = self.wal.get(&k).cloned().expect("key from scan");
            match (rec.state, rec.commit) {
                // Not yet dispatched: safe to resume from Authorized/Planned.
                (EffectState::Planned, _) | (EffectState::Authorized, _) => {
                    // No external effect happened; resumption is safe. Left as-is.
                }
                // Dispatched with unknown commit: reconcile, never re-dispatch.
                (EffectState::Dispatched, CommitStatus::Unknown) => {
                    if rec.auto_retry_eligible() {
                        // Read-back reconciliation (PRD 7.6, Q16).
                        rec.commit = if self.ext.commit_count(&rec.op_key) > 0 {
                            CommitStatus::Committed
                        } else {
                            CommitStatus::NotCommitted
                        };
                        rec.state = EffectState::Reconciled;
                        self.wal.put(rec);
                    } else {
                        // Non-idempotent, no read-back: cannot resolve safely.
                        rec.state = EffectState::Reconciled;
                        // commit remains Unknown -> reconciliation_required.
                        self.wal.put(rec);
                        needs_human += 1;
                    }
                }
                _ => {}
            }
        }
        needs_human
    }
}

impl Default for EffectManager {
    fn default() -> Self {
        Self::new()
    }
}

/// One row of the crash-injection conformance report (RS-1, PRD 8.5).
#[derive(Clone, Debug, PartialEq)]
pub struct ConformanceRow {
    /// The crash point exercised.
    pub crash: CrashPoint,
    /// A short label for the effect profile (idempotent / read-back / neither).
    pub profile: &'static str,
    /// Commit count for the logical effect after crash + recovery.
    pub commit_count: u32,
    /// Whether human reconciliation was required.
    pub reconciliation_required: bool,
    /// Whether the invariant held: no silent duplicate consequential effect.
    pub ok: bool,
}

/// Run the crash-injection conformance suite (RS-1): inject a crash at every
/// effect-state transition for representative effect profiles, recover, and
/// verify no test effect is silently duplicated (PRD 8.5, NFR "Effect correctness").
pub fn crash_injection_conformance() -> Vec<ConformanceRow> {
    let profiles: [(&str, EffectMetadata); 3] = [
        ("idempotent", meta(true, false)),
        ("read-back", meta(false, true)),
        ("neither", meta(false, false)),
    ];
    let crashes = [
        CrashPoint::BeforeIntent,
        CrashPoint::AfterIntent,
        CrashPoint::AfterAuthorize,
        CrashPoint::AfterDispatch,
        CrashPoint::AfterAck,
        CrashPoint::None,
    ];

    let mut rows = Vec::new();
    for (profile, m) in &profiles {
        for &crash in &crashes {
            let mut mgr = EffectManager::new();
            let op = OperationKey::new("conf-run", "root/write");
            let outcome = mgr.attempt(&op, "write_tool", "sha256:req", m, crash);
            // Recovery after a crash. A completed attempt needs no recovery.
            let needs_human = if matches!(outcome, AttemptOutcome::Crashed(_)) {
                mgr.recover()
            } else {
                0
            };
            let commit_count = mgr.external().commit_count(&op.as_string());
            // Invariant: a single logical effect never commits more than once.
            let ok = commit_count <= 1;
            rows.push(ConformanceRow {
                crash,
                profile,
                commit_count,
                reconciliation_required: needs_human > 0,
                ok,
            });
        }
    }
    rows
}

fn meta(idempotent: bool, read_back: bool) -> EffectMetadata {
    EffectMetadata {
        class: EffectClass::Irreversible,
        idempotent,
        reversible: false,
        dry_run_supported: false,
        read_back_supported: read_back,
        attestation: entelechy_ir::AttestationLevel::OperatorAttested,
        operation_key_namespace: "conf".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_silent_duplicate_at_any_crash_point() {
        // The core RS-1 property (PRD 8.5, NFR effect correctness).
        let rows = crash_injection_conformance();
        assert!(rows.iter().all(|r| r.ok), "silent duplicate detected: {:?}",
            rows.iter().filter(|r| !r.ok).collect::<Vec<_>>());
    }

    #[test]
    fn non_idempotent_unknown_commit_needs_reconciliation() {
        // Crash after dispatch on a non-idempotent, no-read-back effect: must not
        // re-dispatch; must require human reconciliation (PRD 8.5).
        let mut mgr = EffectManager::new();
        let op = OperationKey::new("r", "root/pay");
        let m = meta(false, false);
        let outcome = mgr.attempt(&op, "pay", "h", &m, CrashPoint::AfterDispatch);
        assert_eq!(outcome, AttemptOutcome::Crashed(CrashPoint::AfterDispatch));
        let needs_human = mgr.recover();
        assert_eq!(needs_human, 1);
        // Still exactly one commit — no duplicate.
        assert_eq!(mgr.external().commit_count(&op.as_string()), 1);
        assert_eq!(mgr.wal().get(&op.as_string()).unwrap().commit, CommitStatus::Unknown);
    }

    #[test]
    fn idempotent_recovers_by_read_back_without_duplicate() {
        let mut mgr = EffectManager::new();
        let op = OperationKey::new("r", "root/draft");
        let m = meta(true, false);
        mgr.attempt(&op, "draft", "h", &m, CrashPoint::AfterDispatch);
        let needs_human = mgr.recover();
        assert_eq!(needs_human, 0);
        let rec = mgr.wal().get(&op.as_string()).unwrap();
        assert_eq!(rec.state, EffectState::Reconciled);
        assert_eq!(rec.commit, CommitStatus::Committed);
        assert_eq!(mgr.external().commit_count(&op.as_string()), 1);
    }

    #[test]
    fn crash_before_intent_leaves_nothing() {
        let mut mgr = EffectManager::new();
        let op = OperationKey::new("r", "root/pay");
        let m = meta(false, false);
        let outcome = mgr.attempt(&op, "pay", "h", &m, CrashPoint::BeforeIntent);
        assert_eq!(outcome, AttemptOutcome::Crashed(CrashPoint::BeforeIntent));
        assert_eq!(mgr.recover(), 0);
        assert_eq!(mgr.external().commit_count(&op.as_string()), 0);
    }
}
