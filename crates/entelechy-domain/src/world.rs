//! The resettable helpdesk world and its programmatic state checkers (PRD §21,
//! Q1; EV-5 resettable environments).
//!
//! The world is plain mutable state. It knows nothing about the Design IR: the
//! [`crate::exec`] interpreter calls these methods as *effects*, and the
//! [`HelpdeskWorld::resolved`] checker decides success by inspecting only this
//! state. That separation is the whole point — success is not read off the design
//! structure (as the declared simulator did), it is read off what executing the
//! design actually did to the world.
//!
//! Three ticket classes exercise the three Phase-0 repairs: a reply ticket needs a
//! grounded reply (a `Verify` step, level 3); a decomposition ticket needs every
//! part resolved (a part-resolving tool, level 5); and a knowledge ticket needs a
//! durable fact retrieved from semantic memory (a `Retrieval` step, level 2) before
//! its reply means anything.

use std::collections::BTreeSet;

use entelechy_eval::Task;
use entelechy_ir::MemoryTier;

/// The revision the grounding/verification step is calibrated against.
///
/// A `Verify` node grounds a reply only when the reply was drafted by a model
/// whose revision matches this. A silent provider drift to a different revision
/// therefore stops grounding working — the real failure mode PV-2 guards against
/// (see [`crate::epoch`]). The mock gateway pins `mock-r1`, so that is the
/// calibrated baseline.
pub const CALIBRATED_REVISION: &str = "mock-r1";

/// What resolving a ticket requires (derived from a [`Task`]'s tags and input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TicketKind {
    /// The customer needs a grounded reply: resolved once a reply exists and a
    /// verification step has grounded it.
    Reply,
    /// A multi-part request: resolved once every one of its `parts` is addressed.
    Decomposition {
        /// Number of distinct parts that must each be resolved.
        parts: usize,
    },
    /// A knowledge-dependent request: answering it needs a durable fact the agent
    /// does not hold a priori. Resolved once a reply has been drafted *and* the
    /// needed fact has been retrieved from durable (semantic) memory (PRD 12, 11.4
    /// level 2). A design with no retrieval step drafts an ungrounded guess and
    /// leaves the ticket open — the `knowledge.retrieval-miss` failure mode.
    Knowledge,
}

/// A drafted customer reply and whether a verification step has grounded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The reply text the drafting model produced (what the customer would see).
    pub text: String,
    /// The provider revision that drafted it (PV-1 identity carried into state).
    pub model_revision: String,
    /// Whether a verification step has grounded this reply.
    pub grounded: bool,
}

/// The resettable helpdesk world (PRD §21, Q1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpdeskWorld {
    kind: TicketKind,
    calibrated_revision: String,
    reply: Option<Reply>,
    resolved_parts: BTreeSet<usize>,
    retrieved_fact: bool,
    refund_issued: bool,
    refund_attempts: u32,
}

impl HelpdeskWorld {
    /// Build the initial world for a task (this *is* the reset — a fresh world for
    /// the ticket). `needs_reply` tickets need a grounded reply; tickets tagged
    /// `needs_decomposition` read a `parts` count from the task input (default 3).
    pub fn for_task(task: &Task) -> Self {
        let kind = if task.has_tag("needs_decomposition") {
            let parts = task
                .input
                .get("parts")
                .and_then(|p| p.as_u64())
                .unwrap_or(3) as usize;
            TicketKind::Decomposition {
                parts: parts.max(1),
            }
        } else if task.has_tag("needs_retrieval") {
            TicketKind::Knowledge
        } else {
            TicketKind::Reply
        };
        Self::new(kind, CALIBRATED_REVISION)
    }

    /// Build a fresh world for a ticket kind and a calibrated verification revision.
    pub fn new(kind: TicketKind, calibrated_revision: impl Into<String>) -> Self {
        Self {
            kind,
            calibrated_revision: calibrated_revision.into(),
            reply: None,
            resolved_parts: BTreeSet::new(),
            retrieved_fact: false,
            refund_issued: false,
            refund_attempts: 0,
        }
    }

    /// Reset the mutable state to the ticket's initial condition (EV-5), keeping
    /// the immutable ticket kind and calibrated revision.
    pub fn reset(&mut self) {
        self.reply = None;
        self.resolved_parts.clear();
        self.retrieved_fact = false;
        self.refund_issued = false;
        self.refund_attempts = 0;
    }

    /// The ticket kind.
    pub fn kind(&self) -> &TicketKind {
        &self.kind
    }

    /// The current reply, if one has been drafted.
    pub fn reply(&self) -> Option<&Reply> {
        self.reply.as_ref()
    }

    // --- Effects the executor applies (the "tools") -------------------------

    /// Draft an (ungrounded) reply with the given text and drafting-model revision.
    /// Re-drafting replaces the previous reply and clears its grounding.
    pub fn draft_reply(&mut self, text: impl Into<String>, model_revision: impl Into<String>) {
        self.reply = Some(Reply {
            text: text.into(),
            model_revision: model_revision.into(),
            grounded: false,
        });
    }

    /// Run the verification step. It grounds the reply only when a reply exists and
    /// its drafting revision matches the calibrated revision; otherwise grounding
    /// silently does not take (the drift failure mode). Returns whether the reply
    /// is grounded afterwards.
    pub fn verify_reply(&mut self) -> bool {
        let calibrated = self.calibrated_revision.clone();
        match self.reply.as_mut() {
            Some(reply) if reply.model_revision == calibrated => {
                reply.grounded = true;
                true
            }
            _ => false,
        }
    }

    /// Resolve one part of a multi-part ticket (no-op for a reply ticket, or for an
    /// index past the ticket's part count).
    pub fn resolve_part(&mut self, index: usize) {
        if let TicketKind::Decomposition { parts } = self.kind {
            if index < parts {
                self.resolved_parts.insert(index);
            }
        }
    }

    /// A committee resolves every part of a multi-part ticket at once.
    pub fn resolve_all_parts(&mut self) {
        if let TicketKind::Decomposition { parts } = self.kind {
            self.resolved_parts.extend(0..parts);
        }
    }

    /// Retrieve durable knowledge for the ticket (PRD 12, 11.4 level 2). Only a
    /// search of the *semantic* tier with a positive budget surfaces the durable
    /// fact a [`TicketKind::Knowledge`] ticket needs; a misconfigured retrieval
    /// (wrong tier, or `top_k == 0`) finds nothing, so a retrieval step only helps
    /// when it is actually configured to read durable knowledge. Returns whether
    /// the needed fact is now available. (For non-knowledge tickets the flag is
    /// recorded but unused by [`Self::resolved`], so retrieval is a safe no-op.)
    pub fn retrieve(&mut self, tier: MemoryTier, top_k: u32) -> bool {
        if tier == MemoryTier::Semantic && top_k >= 1 {
            self.retrieved_fact = true;
        }
        self.retrieved_fact
    }

    /// Whether the needed durable fact has been retrieved (for the knowledge checker).
    pub fn retrieved_fact(&self) -> bool {
        self.retrieved_fact
    }

    /// Attempt a refund. The world always refuses it (the refund capability is a
    /// structural negative goal, forbidden by the authority envelope), so the
    /// ledger never records one. Returns `false` — the refund did not happen.
    pub fn attempt_refund(&mut self) -> bool {
        self.refund_attempts += 1;
        false
    }

    /// How many parts are resolved so far.
    pub fn resolved_part_count(&self) -> usize {
        self.resolved_parts.len()
    }

    /// How many times a refund was attempted (always refused).
    pub fn refund_attempts(&self) -> u32 {
        self.refund_attempts
    }

    // --- Programmatic state checkers ---------------------------------------

    /// Whether the negative goal held: no refund was ever issued (PRD 5.6
    /// structural negative goal). Always true by construction — the world refuses
    /// every refund — which is exactly the guarantee the checker confirms.
    pub fn negative_goal_held(&self) -> bool {
        !self.refund_issued
    }

    /// The programmatic state checker (EV-4): whether this ticket is resolved.
    ///
    /// Decided purely from world state. The negative goal is checked first (a
    /// refund can never have been issued), then the ticket-specific condition: a
    /// reply ticket needs a grounded reply; a decomposition ticket needs every part
    /// resolved.
    pub fn resolved(&self) -> bool {
        if !self.negative_goal_held() {
            return false;
        }
        match &self.kind {
            TicketKind::Reply => self.reply.as_ref().is_some_and(|r| r.grounded),
            TicketKind::Decomposition { parts } => {
                (0..*parts).all(|i| self.resolved_parts.contains(&i))
            }
            // A knowledge ticket needs both a drafted reply and the durable fact it
            // depends on, retrieved from semantic memory. Grounding is not required
            // (that is the reply class); what distinguishes it is the retrieval.
            TicketKind::Knowledge => self.reply.is_some() && self.retrieved_fact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_ticket_resolves_only_when_grounded() {
        let mut w = HelpdeskWorld::new(TicketKind::Reply, CALIBRATED_REVISION);
        assert!(!w.resolved(), "no reply yet");
        w.draft_reply("hello", CALIBRATED_REVISION);
        assert!(!w.resolved(), "drafted but not grounded");
        assert!(w.verify_reply());
        assert!(w.resolved(), "grounded reply resolves the ticket");
    }

    #[test]
    fn verification_calibrated_to_a_revision_does_not_ground_a_drifted_reply() {
        let mut w = HelpdeskWorld::new(TicketKind::Reply, CALIBRATED_REVISION);
        w.draft_reply("hello", "mock-r2"); // a drifted provider revision
        assert!(
            !w.verify_reply(),
            "verification is not calibrated to mock-r2"
        );
        assert!(!w.resolved(), "a drifted reply is never grounded");
    }

    #[test]
    fn decomposition_resolves_only_when_all_parts_are() {
        let mut w = HelpdeskWorld::new(TicketKind::Decomposition { parts: 3 }, CALIBRATED_REVISION);
        w.resolve_part(0);
        assert!(!w.resolved(), "1/3 parts");
        w.resolve_part(1);
        w.resolve_part(2);
        assert!(w.resolved(), "3/3 parts");
    }

    #[test]
    fn committee_resolves_every_part_at_once() {
        let mut w = HelpdeskWorld::new(TicketKind::Decomposition { parts: 4 }, CALIBRATED_REVISION);
        w.resolve_all_parts();
        assert_eq!(w.resolved_part_count(), 4);
        assert!(w.resolved());
    }

    #[test]
    fn knowledge_ticket_resolves_only_with_a_reply_and_a_semantic_retrieval() {
        let mut w = HelpdeskWorld::new(TicketKind::Knowledge, CALIBRATED_REVISION);
        assert!(!w.resolved(), "nothing drafted or retrieved yet");
        w.draft_reply("a guess", CALIBRATED_REVISION);
        assert!(
            !w.resolved(),
            "an ungrounded guess without the fact is not enough"
        );
        assert!(
            w.retrieve(MemoryTier::Semantic, 4),
            "semantic retrieval surfaces the fact"
        );
        assert!(
            w.resolved(),
            "a drafted reply plus the retrieved fact resolves it"
        );
    }

    #[test]
    fn a_misconfigured_retrieval_does_not_surface_the_fact() {
        let mut w = HelpdeskWorld::new(TicketKind::Knowledge, CALIBRATED_REVISION);
        w.draft_reply("a guess", CALIBRATED_REVISION);
        // Wrong tier: episodic memory is not where durable facts live.
        assert!(!w.retrieve(MemoryTier::Episodic, 4));
        // A zero budget retrieves nothing even from the right tier.
        assert!(!w.retrieve(MemoryTier::Semantic, 0));
        assert!(
            !w.resolved(),
            "a misconfigured retrieval leaves the ticket open"
        );
        // Configured correctly, it now resolves.
        assert!(w.retrieve(MemoryTier::Semantic, 1));
        assert!(w.resolved());
    }

    #[test]
    fn refund_is_always_refused_and_the_negative_goal_holds() {
        let mut w = HelpdeskWorld::new(TicketKind::Reply, CALIBRATED_REVISION);
        assert!(!w.attempt_refund());
        assert!(!w.attempt_refund());
        assert_eq!(w.refund_attempts(), 2);
        assert!(w.negative_goal_held(), "no refund was ever issued");
    }

    #[test]
    fn reset_restores_the_initial_condition() {
        let mut w = HelpdeskWorld::new(TicketKind::Reply, CALIBRATED_REVISION);
        w.draft_reply("x", CALIBRATED_REVISION);
        w.verify_reply();
        assert!(w.resolved());
        w.reset();
        assert!(!w.resolved(), "a reset world is back to unresolved");
        assert!(w.reply().is_none());
    }

    #[test]
    fn ticket_kind_is_derived_from_task_tags_and_input() {
        use entelechy_eval::{Difficulty, Provenance, Split};
        let reply = Task {
            id: "r".into(),
            input: serde_json::json!({}),
            environment: "helpdesk".into(),
            checkers: vec!["resolved".into()],
            tags: vec!["needs_reply".into()],
            difficulty: Difficulty::Medium,
            provenance: Provenance::HumanSeed,
            split: Split::Tune,
            source_task: None,
        };
        assert_eq!(*HelpdeskWorld::for_task(&reply).kind(), TicketKind::Reply);

        let dec = Task {
            id: "d".into(),
            input: serde_json::json!({ "parts": 2 }),
            environment: "helpdesk".into(),
            checkers: vec!["resolved".into()],
            tags: vec!["needs_decomposition".into()],
            difficulty: Difficulty::Medium,
            provenance: Provenance::HumanSeed,
            split: Split::Tune,
            source_task: None,
        };
        assert_eq!(
            *HelpdeskWorld::for_task(&dec).kind(),
            TicketKind::Decomposition { parts: 2 }
        );
    }
}
