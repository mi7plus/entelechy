//! Shadow → governance bridge (PRD §14.2 L3): the staged-rollout shadow stage
//! consumes *runtime-enforced* substitution evidence, not trusted metadata.
//!
//! The governance shadow stage ([`entelechy_release::run_shadow`]) decides whether a
//! candidate may proceed to canary from a set of [`RolloutObservation`]s, each
//! carrying a per-run count of consequential effects the candidate "would" commit.
//! Before this bridge those counts were *supplied* by the caller — the governance
//! layer trusted the harness (and each capability's `dry_run_supported` metadata) to
//! have actually substituted the effects (see the hand-authored
//! `RunOutcome::ok(..).with_effects(1)` in `entelechy_release::rollout_drill`). The
//! WP2 runtime ([`Engine::shadow`]) makes substitution a *runtime guarantee*: it
//! intercepts every consequential tool effect — the tool gateway is never called —
//! and records it, committing nothing by construction.
//!
//! This module closes the gap. For each production input it:
//! 1. drives the candidate through [`Engine::shadow`] and **asserts
//!    `committed_no_effects()`** (and a zero `ToolEffect` count) on the result — the
//!    runtime guarantee, checked rather than assumed;
//! 2. builds the governance observation from the *measured* substitution evidence:
//!    `candidate.consequential_effects` is `ShadowResult::substituted.len()`, and the
//!    candidate's comparison token is the substitution-invariant **effect signature**
//!    it would dispatch — the ordered `(capability, canonical request hash)` list,
//!    which is mode-independent (computed from the node, not the dispatched response),
//!    so it is directly comparable to the baseline's committed signature from a real
//!    [`Engine::execute`];
//! 3. feeds the observations into [`run_shadow`].
//!
//! The governance decision then rests on what the runtime provably did — intercepted
//! N effects and committed none — not on what a capability's metadata claimed.

use serde::{Deserialize, Serialize};

use entelechy_gateway::{MockModel, NativeToolGateway};
use entelechy_ir::{AuthorityEnvelope, Node, NodeKind, Program, ToolNode, Value};
use entelechy_release::{run_shadow, RolloutObservation, RunOutcome, ShadowConfig, ShadowDecision};
use entelechy_runtime::{Engine, Journal, JournalEvent, RunStatus, ShadowResult};

/// Shadow samples the bridge replays — above `ShadowConfig::default().min_samples`
/// (20) so a clean parity run yields a conclusive `Proceed` rather than a
/// too-few-samples hold.
const SAMPLES: usize = 24;

/// One boolean per invariant the shadow → governance bridge enforces, plus `ok`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowGovernanceReport {
    /// Every candidate input driven through [`Engine::shadow`] committed **no**
    /// consequential effect: `committed_no_effects()` held and the shadow journal
    /// recorded zero `ToolEffect`s. This is the runtime guarantee the governance
    /// stage now relies on, asserted rather than assumed.
    pub runtime_shadow_commits_nothing: bool,
    /// The *same* candidate program run through [`Engine::execute`] (the active
    /// baseline path) **does** commit its consequential effect — the before/after
    /// contrast that makes the substitution meaningful (the program is not a no-op).
    pub execute_commits_the_effect: bool,
    /// The governance report's `candidate_effects_substituted` equals the count the
    /// runtime actually intercepted across the inputs, and is non-zero: the figure is
    /// *measured* from `ShadowResult::substituted`, not supplied by the caller.
    pub substitution_count_is_runtime_measured: bool,
    /// Fed runtime-sourced observations whose effect signatures match the baseline's,
    /// the governance shadow stage clears the candidate for canary (`Proceed`).
    pub governance_proceeds_on_parity: bool,
    /// A candidate whose runtime-shadowed effect signature diverges from the
    /// baseline's is held by the governance stage — it still discriminates over
    /// runtime evidence, and the diverging candidate *also* commits nothing in shadow.
    pub governance_holds_on_divergence: bool,
    /// Whether every bridge invariant held.
    pub ok: bool,
}

/// Build the shadow → governance report (PRD §14.2 L3). Deterministic: fixed
/// programs, inputs and the mock model mean the same inputs always yield the same
/// report.
pub fn shadow_governance_report() -> ShadowGovernanceReport {
    let model = MockModel::new();

    // Parity case: the candidate is the same single-effect program as the active
    // baseline, so their effect signatures match and the stage should proceed.
    let candidate = tool_program("publish");
    let baseline = tool_program("publish");

    let mut shadow_commits_nothing = true;
    let mut execute_commits = true;
    let mut measured_substituted: u64 = 0;

    let observations: Vec<RolloutObservation> = (0..SAMPLES)
        .map(|i| {
            let run_id = format!("shadow-gov-{i}");
            let input = Value::trusted(serde_json::json!({ "ticket": i }));

            // (1) Candidate via Engine::shadow — intercept + prove nothing commits.
            let sh = {
                let mut tools = gateway("publish");
                let mut engine = Engine::new(&model, &mut tools);
                engine.shadow(&candidate, input.clone(), &run_id)
            };
            if !(sh.committed_no_effects()
                && sh.run.journal.tool_effect_count() == 0
                && sh.run.status == RunStatus::Succeeded)
            {
                shadow_commits_nothing = false;
            }
            measured_substituted += sh.substituted.len() as u64;

            // Baseline via Engine::execute — the active release actually commits.
            let base = {
                let mut tools = gateway("publish");
                let mut engine = Engine::new(&model, &mut tools);
                engine.execute(&baseline, input, &run_id)
            };
            if !(base.journal.tool_effect_count() >= 1 && base.status == RunStatus::Succeeded) {
                execute_commits = false;
            }

            // (2) Governance observation from measured evidence; the comparison token
            //     is the substitution-invariant effect signature.
            observation(
                run_id,
                committed_signature(&base.journal),
                base.journal.tool_effect_count() as u32,
                substituted_signature(&sh),
                sh.substituted.len() as u32,
            )
        })
        .collect();

    // (3) Feed the governance shadow stage.
    let report = run_shadow(&ShadowConfig::default(), &observations);

    let substitution_count_is_runtime_measured = report.candidate_effects_substituted
        == measured_substituted
        && report.candidate_effects_substituted == SAMPLES as u64
        && measured_substituted > 0;
    let governance_proceeds_on_parity =
        report.decision == ShadowDecision::Proceed && report.divergences == 0;

    // Divergence case: the candidate dispatches a *different* capability than the
    // baseline, so every input's signature differs. It must still commit nothing in
    // shadow, and the governance stage must hold.
    let div_candidate = tool_program("notify");
    let mut div_shadow_commits_nothing = true;
    let div_observations: Vec<RolloutObservation> = (0..SAMPLES)
        .map(|i| {
            let run_id = format!("shadow-gov-div-{i}");
            let input = Value::trusted(serde_json::json!({ "ticket": i }));
            let sh = {
                let mut tools = gateway("notify");
                let mut engine = Engine::new(&model, &mut tools);
                engine.shadow(&div_candidate, input.clone(), &run_id)
            };
            if !(sh.committed_no_effects() && sh.run.journal.tool_effect_count() == 0) {
                div_shadow_commits_nothing = false;
            }
            let base = {
                let mut tools = gateway("publish");
                let mut engine = Engine::new(&model, &mut tools);
                engine.execute(&baseline, input, &run_id)
            };
            observation(
                run_id,
                committed_signature(&base.journal),
                base.journal.tool_effect_count() as u32,
                substituted_signature(&sh),
                sh.substituted.len() as u32,
            )
        })
        .collect();
    let div_report = run_shadow(&ShadowConfig::default(), &div_observations);
    let governance_holds_on_divergence = matches!(div_report.decision, ShadowDecision::Hold { .. })
        && div_report.divergences == SAMPLES
        && div_shadow_commits_nothing;

    let ok = shadow_commits_nothing
        && execute_commits
        && substitution_count_is_runtime_measured
        && governance_proceeds_on_parity
        && governance_holds_on_divergence;

    ShadowGovernanceReport {
        runtime_shadow_commits_nothing: shadow_commits_nothing,
        execute_commits_the_effect: execute_commits,
        substitution_count_is_runtime_measured,
        governance_proceeds_on_parity,
        governance_holds_on_divergence,
        ok,
    }
}

/// An authority envelope granting exactly `caps`.
fn auth(caps: &[&str]) -> AuthorityEnvelope {
    AuthorityEnvelope {
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// A one-node program that invokes `cap` as a consequential tool effect, granting
/// exactly that capability.
fn tool_program(cap: &str) -> Program {
    Program::new(
        auth(&[cap]),
        Node::new(
            "send",
            NodeKind::Tool(ToolNode {
                capability: cap.into(),
                args: serde_json::Value::Null,
            }),
        ),
    )
}

/// A tool gateway that registers `cap` with a record-only success response. Used so
/// the baseline `execute` has a tool to dispatch; the candidate `shadow` never calls
/// it.
fn gateway(cap: &'static str) -> NativeToolGateway {
    let mut tools = NativeToolGateway::new();
    tools.register(cap, move |_| Ok(serde_json::json!({ "ok": true })));
    tools
}

/// The ordered `(capability, request_hash)` signature of the consequential effects a
/// committed run actually journaled — comparable to a shadow run's substituted
/// signature because `request_hash` is computed from the node, independent of mode.
fn committed_signature(journal: &Journal) -> Vec<(String, String)> {
    journal
        .entries
        .iter()
        .filter_map(|e| match &e.event {
            JournalEvent::ToolEffect {
                capability,
                request_hash,
                ..
            } => Some((capability.clone(), request_hash.clone())),
            _ => None,
        })
        .collect()
}

/// The ordered `(capability, request_hash)` signature of the effects a shadow run
/// intercepted and substituted (what it *would* have dispatched).
fn substituted_signature(sh: &ShadowResult) -> Vec<(String, String)> {
    sh.substituted
        .iter()
        .map(|s| (s.capability.clone(), s.request_hash.clone()))
        .collect()
}

/// Build one rollout observation: the baseline's committed signature/count and the
/// candidate's substituted signature/count. The result token is the effect
/// signature, so parity means "the candidate would dispatch the effects the baseline
/// committed".
fn observation(
    run_id: String,
    baseline_sig: Vec<(String, String)>,
    baseline_effects: u32,
    candidate_sig: Vec<(String, String)>,
    candidate_effects: u32,
) -> RolloutObservation {
    RolloutObservation {
        run_id,
        baseline: RunOutcome::ok(sig_token(&baseline_sig), 10.0).with_effects(baseline_effects),
        candidate: RunOutcome::ok(sig_token(&candidate_sig), 10.0).with_effects(candidate_effects),
    }
}

/// A deterministic comparison token for an effect signature.
fn sig_token(sig: &[(String, String)]) -> String {
    serde_json::to_string(sig).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_satisfies_every_invariant() {
        let r = shadow_governance_report();
        assert!(
            r.ok,
            "every shadow→governance bridge invariant must hold: {r:?}"
        );
        assert!(r.runtime_shadow_commits_nothing);
        assert!(r.execute_commits_the_effect);
        assert!(r.substitution_count_is_runtime_measured);
        assert!(r.governance_proceeds_on_parity);
        assert!(r.governance_holds_on_divergence);
    }

    #[test]
    fn report_is_deterministic() {
        assert_eq!(shadow_governance_report(), shadow_governance_report());
    }

    #[test]
    fn candidate_shadow_signature_matches_baseline_execute_signature() {
        // The invariant the parity check rests on: for the same program, the
        // substituted signature (shadow) equals the committed signature (execute).
        let model = MockModel::new();
        let prog = tool_program("publish");
        let input = Value::trusted(serde_json::json!({ "ticket": 7 }));

        let sh = {
            let mut tools = gateway("publish");
            let mut engine = Engine::new(&model, &mut tools);
            engine.shadow(&prog, input.clone(), "sig-shadow")
        };
        let base = {
            let mut tools = gateway("publish");
            let mut engine = Engine::new(&model, &mut tools);
            engine.execute(&prog, input, "sig-exec")
        };

        let shadow_sig = substituted_signature(&sh);
        let committed_sig = committed_signature(&base.journal);
        assert_eq!(
            shadow_sig, committed_sig,
            "signatures must match for parity"
        );
        assert_eq!(shadow_sig.len(), 1);
        assert_eq!(shadow_sig[0].0, "publish");
        // Shadow committed nothing; execute committed exactly one.
        assert!(sh.committed_no_effects());
        assert_eq!(base.journal.tool_effect_count(), 1);
    }
}
