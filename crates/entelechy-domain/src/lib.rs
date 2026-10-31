//! The Phase-0 real-domain study: a resettable helpdesk executed against mutable
//! world state, with programmatic state checkers.
//!
//! PRD v11 references: section 21 (Phase 0 roadmap and the "simulated helpdesk
//! with programmatic state checkers", Q1), 21.1 (study protocol), 5.10 (model
//! identity, PV-1/PV-2), 17.1 (deployment profile, DP-1).
//!
//! # Why this crate exists
//!
//! The original `entelechy study` demo is a *declared simulator*: it decides a
//! task's success by inspecting the *design structure* (does the IR contain a
//! `Verify` node? a `Par` node?). That is circular — the thing being optimized is
//! also the thing being scored — and it produces no evidence that a real domain,
//! executed against mutable state, actually resolves the ticket.
//!
//! This crate replaces that with a genuine domain:
//!
//! 1. [`HelpdeskWorld`] is **resettable** mutable state (EV-5): a knowledge base,
//!    a drafted reply with its grounding status, a set of resolved ticket parts,
//!    and a refund ledger.
//! 2. [`execute`] is an **interpreter**: it walks the Design IR and applies each
//!    node's *effect* to the world — an `Llm` node drafts an (ungrounded) reply, a
//!    `Verify` node grounds it, a `Par` committee resolves every part, a `refund`
//!    tool is refused. The design is run, not read.
//! 3. [`resolves`] decides success with a **programmatic state checker** that
//!    inspects only the final [`HelpdeskWorld`] — never the program. Scoring is
//!    therefore independent of the structure that produced the state.
//!
//! On top of that domain the study produces the Phase-0 evidence the TR-1 gate
//! still waives:
//!
//! - **PV-2** (no silent pooling across material drift): every model call's
//!   [`ModelFingerprint`] keys an [`EvidenceEpoch`]; a material drift (a changed
//!   provider revision) opens a new epoch, and the release decision reads only the
//!   current epoch's [`EvidenceLedger`], so pre-drift successes can never be pooled
//!   with post-drift evidence.
//! - **DP-1** (one declared deployment profile): [`DeploymentDeclaration`] names
//!   exactly one [`entelechy_artifacts::DeploymentProfile`] and carries a formal,
//!   signed approval of it — the single-deployment-profile approval step.
//! - **PV-3** (drift beyond tolerance blocks automatic promotion): [`drift_drill`]
//!   reads the behavioral regression between the pre-drift and post-drift evidence
//!   epochs and blocks the automatic promotion path when it exceeds a declared
//!   [`DriftTolerance`]; a within-tolerance drift and a recorded human override do
//!   not (PRD 5.10, §21).
//!
//! The whole run is captured in a content-addressed [`ReproductionPacket`] so a
//! re-run reproduces the same resolved counts bit-for-bit (determinism).
//!
//! Finally, [`searched::searched_study`] unifies this real-domain study with the
//! simulated `entelechy study`: it drives the *same* design-repair proposer loop
//! the simulated study uses, but scores each candidate by **executing** it here
//! instead of by reading its structure. The loop discovers the verification repair
//! and the executor confirms it works; a parallel committee that the structural
//! scorer would credit is rejected because running it resolves nothing. See that
//! module's docs for why that divergence is the whole point.
//!
//! That unification closes the loop back onto [`study::real_study`]:
//! [`searched::discovered_repair`] exposes the design the loop converges on, and
//! `real_study` now consumes *that* searched design as its candidate — proving it
//! resolves every suite ticket exactly as the retired hand-authored
//! [`study::repaired_design`] oracle did (PRD §11.3). No hand-authored repair
//! remains on a live study path.
#![forbid(unsafe_code)]

pub mod epoch;
pub mod exec;
pub mod promotion;
pub mod searched;
pub mod study;
pub mod world;

pub use epoch::{EvidenceEpoch, EvidenceLedger, ModelFingerprint, RevisionModel};
pub use exec::{compare, execute, resolves, ComparisonReport, Execution, TaskDelta};
pub use promotion::{
    assess_drift, auto_promote, drift_drill, promotion_proceeds, AutoPromotionDecision,
    DriftAssessment, DriftDrillReport, DriftTolerance,
};
pub use searched::{discovered_repair, searched_study, SearchedStudyReport};
pub use study::{
    baseline_design, helpdesk_suite, real_study, refund_attempting_design, repaired_design,
    DeploymentDeclaration, RealStudyReport, ReproductionError, ReproductionPacket,
};
pub use world::{HelpdeskWorld, Reply, TicketKind};
