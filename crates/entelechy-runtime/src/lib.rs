//! Runtime kernel: interpreter, journal and deterministic replay.
//!
//! PRD v11 references: sections 8 (runtime kernel & replay), 8.6 (failure /
//! terminal states), RK-1..RK-8.
//!
//! The same interpreter drives search, replay and production (PRD 3.1).
//! Execution journals every external effect (RK-2); recorded-effect replay reruns
//! the IR against the journal and must reproduce observable state exactly
//! (PRD 8.2, 18). A run always ends in a typed terminal state derived from
//! journal state (PRD 8.6).
#![forbid(unsafe_code)]

pub mod cluster;
pub mod codegen;
pub mod exec;
pub mod journal;
pub mod memory;

use std::collections::HashMap;

pub use cluster::{InMemoryQueue, LeasedItem, Priority, WorkItem, WorkQueue, WorkerPool};
pub use codegen::{
    observable_equivalent, CodegenEvidence, CompileError, CompiledPlan, CompiledStep,
};
pub use exec::CounterfactualResult;
pub use memory::TieredMemory;

// Execution internals live in `exec`; `egress_output` is also used by `codegen`.
pub(crate) use exec::egress_output;
use exec::{exec_node, outcome_output, Counters, Mode};

use entelechy_gateway::{ModelGateway, ToolGateway};
use entelechy_ir::{
    AuthorityEnvelope, Confidentiality, EffectMetadata, MemoryTier, Program, Taint, Value,
};
use entelechy_policy::{Decision, DecisionLog, PolicyEngine, PolicyRequest, PolicySnapshot};

pub use journal::{
    CommitStatus, Journal, JournalEvent, JournalReader, ReplayMismatch, JOURNAL_FORMAT_VERSION,
};

/// A registered deterministic code function (PRD 7.1 Code node).
pub type CodeFn = Box<dyn Fn(&Value) -> Result<serde_json::Value, String> + Send>;
/// A registered checker for Verify nodes (PRD 7.1).
pub type CheckerFn = Box<dyn Fn(&Value) -> bool + Send>;

/// Hard execution budgets (PRD 11.6, RK; budget exhaustion is a typed terminal
/// state — PRD 8.6).
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Maximum model calls per run.
    pub max_model_calls: u32,
    /// Maximum tool calls per run.
    pub max_tool_calls: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_model_calls: 1_000,
            max_tool_calls: 1_000,
        }
    }
}

/// The typed terminal state of a run (PRD 8.6).
#[derive(Clone, Debug, PartialEq)]
pub enum RunStatus {
    /// Completed successfully.
    Succeeded,
    /// A typed failure (node id + reason).
    Failed {
        /// Node path where the failure occurred.
        node_path: String,
        /// Typed reason.
        reason: FailureReason,
    },
    /// A budget was exhausted (PRD 8.6 — never converted to a model failure).
    BudgetExhausted {
        /// Which budget was exhausted.
        budget: &'static str,
    },
    /// One or more unknown-commit effects require reconciliation before the run
    /// is complete (PRD 8.5, 8.6). Outputs are withheld.
    ReconciliationRequired,
}

/// Typed failure reasons (PRD 8.6 distinguishes failure kinds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureReason {
    /// A gate did not open.
    GateBlocked(String),
    /// A model call failed.
    ModelError(String),
    /// A tool call failed.
    ToolError(String),
    /// A verification step failed.
    VerificationFailed(String),
    /// A code function failed.
    CodeError(String),
    /// The policy engine denied the effect at its boundary (PRD 8.3).
    PolicyDenied(String),
    /// A node kind is not supported in this runtime slice.
    Unsupported(String),
    /// A schema/shape error in the value flow.
    SchemaError(String),
}

/// The result of a run.
#[derive(Clone, Debug)]
pub struct RunResult {
    /// Terminal status.
    pub status: RunStatus,
    /// Final output value, present only when `status` is `Succeeded`.
    pub output: Option<Value>,
    /// The execution journal.
    pub journal: Journal,
}

/// A halt condition propagated up through the interpreter.
pub(crate) enum Halt {
    Terminal(RunStatus),
}

/// Optional policy enforcement configuration for the runtime (PRD 8.3).
///
/// When present, every consequential tool effect is authorized at its boundary by
/// the [`PolicyEngine`] before dispatch, and denials become a typed
/// [`FailureReason::PolicyDenied`]. Decisions are recorded in a [`DecisionLog`]
/// (kept off the replay journal so replay stays byte-identical; policy is
/// deterministic and re-derivable).
pub struct PolicyConfig {
    /// The policy engine (e.g. the native core, or a Cedar adapter).
    pub engine: Box<dyn PolicyEngine + Send>,
    /// The system authority envelope authorization is checked against (PRD 5.4).
    pub authority: AuthorityEnvelope,
    /// The policy snapshot in force (PRD 5.2).
    pub snapshot: PolicySnapshot,
    /// Effect metadata per capability (PRD 7.6); unknown capabilities default
    /// conservative (CD-7).
    pub catalog: HashMap<String, EffectMetadata>,
    /// The principal effects are attributed to (PRD 5.8).
    pub principal: String,
}

/// The interpreter engine (PRD 8.1). Holds gateways and registries.
pub struct Engine<'a> {
    model: &'a dyn ModelGateway,
    tools: &'a mut dyn ToolGateway,
    checkers: HashMap<String, CheckerFn>,
    code: HashMap<String, CodeFn>,
    memory: TieredMemory,
    budget: Budget,
    policy: Option<PolicyConfig>,
    policy_decisions: DecisionLog,
}

impl<'a> Engine<'a> {
    /// Build an engine over the given gateways.
    pub fn new(model: &'a dyn ModelGateway, tools: &'a mut dyn ToolGateway) -> Self {
        Self {
            model,
            tools,
            checkers: HashMap::new(),
            code: HashMap::new(),
            memory: TieredMemory::new(),
            budget: Budget::default(),
            policy: None,
            policy_decisions: DecisionLog::new(),
        }
    }

    /// Set execution budgets.
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    /// Enable policy enforcement at effect boundaries (PRD 8.3).
    pub fn with_policy(mut self, policy: PolicyConfig) -> Self {
        self.policy = Some(policy);
        self
    }

    /// The recorded policy decisions (PRD 8.3 records decisions).
    pub fn policy_decisions(&self) -> &DecisionLog {
        &self.policy_decisions
    }

    /// Authorize a tool effect at its boundary (PRD 8.3). Returns a typed
    /// [`FailureReason`] on denial or an unmet obligation (fail closed). A no-op
    /// when policy enforcement is not configured.
    fn authorize_tool(&mut self, capability: &str, taint: Taint) -> Result<(), FailureReason> {
        // Evaluate under an immutable borrow that ends before we record.
        let outcome: Option<(Decision, String)> = self.policy.as_ref().map(|pc| {
            let meta = pc
                .catalog
                .get(capability)
                .cloned()
                .unwrap_or_else(|| EffectMetadata::conservative_default(capability));
            let req = PolicyRequest {
                principal: pc.principal.clone(),
                capability: capability.to_string(),
                effect_class: meta.class,
                taint,
                // A gate declassifies taint to Trusted; a still-tainted value here
                // never crossed a Gate (IR-I3).
                gate_satisfied: taint == Taint::Trusted,
                egress_host: None,
                data_classification: None,
                provider: None,
            };
            (
                pc.engine.evaluate(&pc.authority, &pc.snapshot, &req),
                capability.to_string(),
            )
        });

        if let Some((decision, cap)) = outcome {
            let allowed = decision.allowed;
            let reason = decision.reason.clone();
            let unmet_obligation = decision.obligations.first().map(|o| format!("{o:?}"));
            self.policy_decisions.record(cap.clone(), decision);
            if !allowed {
                return Err(FailureReason::PolicyDenied(reason));
            }
            // Phase 0 runtime cannot discharge approval obligations inline, so an
            // outstanding obligation fails closed (PRD 5.4).
            if let Some(o) = unmet_obligation {
                return Err(FailureReason::PolicyDenied(format!(
                    "unmet obligation {o} for '{cap}'"
                )));
            }
        }
        Ok(())
    }

    /// Authorize a model call as an egress effect (PRD 7.5, IR-I9). A value with a
    /// confidentiality label may enter a prompt only if the selected provider is
    /// approved for that label; enforcement happens before dispatch (equivalent to
    /// ModelGateway routing-time enforcement). A no-op when policy is not
    /// configured or the value carries no confidentiality labels.
    fn authorize_model(
        &mut self,
        model: &str,
        confidentiality: &Confidentiality,
    ) -> Result<(), FailureReason> {
        if confidentiality.is_empty() {
            return Ok(());
        }
        let Some(pc) = self.policy.as_ref() else {
            return Ok(());
        };
        let mut denied: Option<String> = None;
        for label in confidentiality {
            if !pc.snapshot.provider_approval.is_approved(label, model) {
                denied = Some(format!(
                    "provider '{model}' not approved for confidentiality '{label}' (PRD 7.5/Q19)"
                ));
                break;
            }
        }
        let version = pc.snapshot.version;
        let decision = Decision {
            allowed: denied.is_none(),
            reason: denied.clone().unwrap_or_else(|| "egress authorized".into()),
            obligations: vec![],
            snapshot_version: version,
        };
        self.policy_decisions
            .record(format!("model:{model}"), decision);
        match denied {
            Some(reason) => Err(FailureReason::PolicyDenied(reason)),
            None => Ok(()),
        }
    }

    /// Pre-seed a memory tier with a known value before a run — e.g. the durable
    /// semantic knowledge base a [`entelechy_ir::RetrievalNode`] reads, or shared
    /// blackboard context for coordinating agents (PRD 12, MK-2). The full value
    /// envelope is kept, so the seeded entry's taint and confidentiality propagate
    /// into whatever retrieves it (MK-7, IR-I9).
    pub fn seed_memory(&mut self, tier: MemoryTier, key: impl Into<String>, value: Value) {
        self.memory.write(tier, key, value);
    }

    /// Register a checker for Verify nodes.
    pub fn register_checker(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&Value) -> bool + Send + 'static,
    ) {
        self.checkers.insert(name.into(), Box::new(f));
    }

    /// Register a deterministic code function.
    pub fn register_code(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(&Value) -> Result<serde_json::Value, String> + Send + 'static,
    ) {
        self.code.insert(name.into(), Box::new(f));
    }

    /// Execute a program, journaling every external effect (RK-2).
    pub fn execute(&mut self, program: &Program, input: Value, run_id: &str) -> RunResult {
        let mut journal = Journal::new(run_id);
        // Record the initial input so replay/counterfactual reconstruct from the
        // real input rather than an empty object (RK-2).
        journal.input = input.clone();
        let mut counters = Counters::default();
        let mut mode = Mode::Execute {
            journal: &mut journal,
            counters: &mut counters,
        };
        let outcome = exec_node(self, &program.root, "root", input, &mut mode);

        // Terminal status derived from journal + outcome (PRD 8.6).
        let status = match outcome {
            Ok(_) if journal.has_unknown_commit() => RunStatus::ReconciliationRequired,
            Ok(_) => RunStatus::Succeeded,
            Err(Halt::Terminal(s)) => s,
        };
        let output = match &status {
            RunStatus::Succeeded => outcome_output(self, program, run_id, &journal),
            _ => None,
        };
        RunResult {
            status,
            output,
            journal,
        }
    }

    /// Replay a program against a recorded journal (PRD 8.2). Returns the
    /// recomputed output, or a divergence point (RK-8).
    ///
    /// Replay reconstructs from the journal's recorded initial input (RK-2). The
    /// supplied `input` is verified against it: a mismatch is reported as
    /// [`ReplayMismatch::InputMismatch`] rather than run, since a faithful replay
    /// must start from the recorded input (replaying a different input cannot
    /// reproduce the run). A version-1 journal has no recorded input, so its input
    /// defaults to `{}`; pass `{}` (or the journal's `input`) to replay it.
    pub fn replay(
        &mut self,
        program: &Program,
        input: Value,
        journal: &Journal,
    ) -> Result<Option<Value>, ReplayMismatch> {
        if input != journal.input {
            return Err(ReplayMismatch::InputMismatch);
        }
        // `input` is verified equal to the recorded input, so replay from it.
        let mut reader = JournalReader::new(journal);
        let mut divergence: Option<ReplayMismatch> = None;
        let mut mode = Mode::Replay {
            reader: &mut reader,
            divergence: &mut divergence,
        };
        let outcome = exec_node(self, &program.root, "root", input, &mut mode);
        if let Some(d) = divergence {
            return Err(d);
        }
        match outcome {
            Ok(v) => Ok(Some(v)),
            Err(Halt::Terminal(_)) => Ok(None),
        }
    }

    /// Counterfactual replay (PRD 10.2, RK-8): replay recorded effects until the
    /// `intervention_path` node, substitute `intervention_value` as that node's
    /// output, then re-execute everything downstream live. Reports the divergence
    /// point and the rollout count (a single rollout is weak causal evidence — the
    /// failure analyzer treats it accordingly, PRD 10.2).
    ///
    /// The pre-divergence span is seeded from the journal's recorded initial input
    /// (RK-2), so the counterfactual branches off the *real* recorded trajectory
    /// rather than an empty object (a version-1 journal has no recorded input, so
    /// it defaults to `{}`).
    pub fn counterfactual(
        &mut self,
        program: &Program,
        journal: &Journal,
        intervention_path: &str,
        intervention_value: Value,
    ) -> CounterfactualResult {
        let mut reader = JournalReader::new(journal);
        let mut diverged = false;
        let mut divergence_point: Option<String> = None;
        let mut rollouts: u32 = 0;
        let output = {
            let mut mode = Mode::Counterfactual {
                reader: &mut reader,
                diverged: &mut diverged,
                divergence_point: &mut divergence_point,
                rollouts: &mut rollouts,
                intervention_path,
                intervention_value: &intervention_value,
            };
            exec_node(
                self,
                &program.root,
                "root",
                journal.input.clone(),
                &mut mode,
            )
            .ok()
        };
        CounterfactualResult {
            divergence_point,
            rollouts,
            diverged,
            output,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_gateway::{MockModel, NativeToolGateway};
    use entelechy_ir::{AuthorityEnvelope, CodeNode, LlmNode, Node, NodeKind, Program, VerifyNode};

    fn demo_program() -> Program {
        Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "classify",
                        NodeKind::Llm(LlmNode {
                            model: "mock".into(),
                            prompt_template: "classify: {input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new(
                        "tag",
                        NodeKind::Code(CodeNode {
                            function: "tag_ok".into(),
                        }),
                    ),
                    Node::new(
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "has_text".into(),
                        }),
                    ),
                ]),
            ),
        )
    }

    fn engine<'a>(model: &'a MockModel, tools: &'a mut NativeToolGateway) -> Engine<'a> {
        let mut e = Engine::new(model, tools);
        e.register_code("tag_ok", |v| {
            let mut obj = v.data.clone();
            if let Some(o) = obj.as_object_mut() {
                o.insert("ok".into(), serde_json::json!(true));
            }
            Ok(obj)
        });
        e.register_checker("has_text", |v| v.data.get("text").is_some());
        e
    }

    #[test]
    fn execute_succeeds_and_journals() {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = engine(&model, &mut tools);
        let result = e.execute(
            &demo_program(),
            Value::trusted(serde_json::json!({})),
            "run-1",
        );
        assert_eq!(result.status, RunStatus::Succeeded, "{:?}", result.status);
        // One model call journaled.
        assert_eq!(result.journal.entries.len(), 1);
    }

    // ===== WP1 · WS1: Par/Map must not launder information-flow labels =====
    //
    // Each probe rides the live Execute path via a Verify checker over `v.meta`, so
    // `run.status` reflects the real threaded input (independent of the separate
    // WS4 "replay-from-{}" output-recomputation gap). Before WS1 every one of these
    // ends in `Failed` because `Par` force-taints + drops confidentiality and `Map`
    // relabels items/result `Trusted`.

    #[test]
    fn par_joins_input_and_child_labels_without_forcing_taint() {
        // IR-I9 / PRD 7.5: a Par output carries the join of the input's and every
        // child's labels — confidentiality is not dropped, and an all-trusted fan-out
        // stays Trusted (no over-tainting).
        use entelechy_ir::{
            CodeNode, MemNode, MemOp, MemoryTier, Node, NodeKind, Program, VerifyNode,
        };

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("echo", |v| Ok(v.data.clone()));
        e.register_checker("par_ok", |v| {
            v.meta.taint == Taint::Trusted
                && v.meta.confidentiality.contains("pii")
                && v.meta.confidentiality.contains("secret")
        });

        // A trusted memory entry carrying its own confidentiality label.
        let mut secret = Value::trusted(serde_json::json!("ledger-row"));
        secret.meta.confidentiality.insert("secret".into());
        e.seed_memory(MemoryTier::Working, "s", secret);

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "fanout",
                        NodeKind::Par(vec![
                            Node::new(
                                "read",
                                NodeKind::Mem(MemNode {
                                    op: MemOp::Read,
                                    key: "s".into(),
                                    tier: MemoryTier::Working,
                                }),
                            ),
                            Node::new(
                                "copy",
                                NodeKind::Code(CodeNode {
                                    function: "echo".into(),
                                }),
                            ),
                        ]),
                    ),
                    Node::new(
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "par_ok".into(),
                        }),
                    ),
                ]),
            ),
        );

        let mut input = Value::trusted(serde_json::json!({ "x": 1 }));
        input.meta.confidentiality.insert("pii".into());
        let run = e.execute(&prog, input, "run-par");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    }

    #[test]
    fn map_item_inherits_collection_taint() {
        // IR-I9: a mapped element is no more trusted than its collection. Before WS1,
        // `Map` wrapped items `Value::trusted`, so a tainted-item check failed.
        use entelechy_ir::{Node, NodeKind, Program, VerifyNode};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        e.register_checker("item_is_tainted", |v| v.meta.taint == Taint::Tainted);

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Map {
                    over: "items".into(),
                    body: Box::new(Node::new(
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "item_is_tainted".into(),
                        }),
                    )),
                },
            ),
        );

        let input = Value::tainted(serde_json::json!({ "items": [{ "v": 1 }, { "v": 2 }] }));
        let run = e.execute(&prog, input, "run-map-item");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    }

    #[test]
    fn map_result_preserves_collection_taint() {
        // IR-I9: the Map result joins the collection's labels; it is not relabeled
        // Trusted (the laundering that WS1 removes).
        use entelechy_ir::{CodeNode, Node, NodeKind, Program, VerifyNode};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("echo", |v| Ok(v.data.clone()));
        e.register_checker("result_tainted", |v| v.meta.taint == Taint::Tainted);

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "map",
                        NodeKind::Map {
                            over: "items".into(),
                            body: Box::new(Node::new(
                                "copy",
                                NodeKind::Code(CodeNode {
                                    function: "echo".into(),
                                }),
                            )),
                        },
                    ),
                    Node::new(
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "result_tainted".into(),
                        }),
                    ),
                ]),
            ),
        );

        let input = Value::tainted(serde_json::json!({ "items": [1, 2] }));
        let run = e.execute(&prog, input, "run-map-res");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
    }

    #[test]
    fn map_cannot_launder_taint_past_a_policy_gate() {
        // Adversarial (IR-I3 + IR-I9): mapping a tainted collection and feeding the
        // result to a consequential Tool must be denied exactly as the raw tainted
        // value would be. Before WS1, Map laundered it to Trusted and the egress was
        // wrongly authorized. A trusted collection is the positive control (guards
        // against over-tainting).
        use entelechy_ir::{
            AttestationLevel, CodeNode, EffectClass, EffectMetadata, Node, NodeKind, Program,
            ToolNode,
        };
        use entelechy_policy::{NativePolicy, PolicySnapshot};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        tools.register("publish", |_| Ok(serde_json::json!({ "published": true })));

        let mut authority = AuthorityEnvelope::empty();
        authority.capabilities.insert("publish".into()); // granted…
        let mut catalog = HashMap::new();
        catalog.insert(
            "publish".to_string(),
            EffectMetadata {
                class: EffectClass::Write, // …but consequential, with no approval obligation
                idempotent: false,
                reversible: true,
                dry_run_supported: false,
                read_back_supported: false,
                attestation: AttestationLevel::OperatorAttested,
                operation_key_namespace: "pub".into(),
            },
        );

        let mut e = Engine::new(&model, &mut tools).with_policy(PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: authority.clone(),
            snapshot: PolicySnapshot::default(),
            catalog,
            principal: "runtime".into(),
        });
        e.register_code("echo", |v| Ok(v.data.clone()));

        let prog = Program::new(
            authority,
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "fanout",
                        NodeKind::Map {
                            over: "items".into(),
                            body: Box::new(Node::new(
                                "copy",
                                NodeKind::Code(CodeNode {
                                    function: "echo".into(),
                                }),
                            )),
                        },
                    ),
                    Node::new(
                        "send",
                        NodeKind::Tool(ToolNode {
                            capability: "publish".into(),
                            args: serde_json::Value::Null,
                        }),
                    ),
                ]),
            ),
        );

        // Positive control: a trusted collection may publish (capability is granted).
        let ok = e.execute(
            &prog,
            Value::trusted(serde_json::json!({ "items": [1] })),
            "run-ok",
        );
        assert_eq!(
            ok.status,
            RunStatus::Succeeded,
            "trusted must pass: {:?}",
            ok.status
        );

        // The attack: a tainted collection must be denied at the egress — Map must
        // not launder it to Trusted.
        let denied = e.execute(
            &prog,
            Value::tainted(serde_json::json!({ "items": [1] })),
            "run-attack",
        );
        assert!(
            matches!(
                denied.status,
                RunStatus::Failed {
                    reason: FailureReason::PolicyDenied(_),
                    ..
                }
            ),
            "tainted must be denied at egress, got {:?}",
            denied.status
        );
        assert!(denied.output.is_none());
    }

    // ===== WP1 · WS4: replay/outcome reconstruct from the recorded input =====
    //
    // Before WS4 the journal never persisted the run input and `outcome_output`
    // re-ran from a hardcoded `{}` (RK-2 broken): a node that reads the input
    // produced a different recomputed output than the live run, and a replay could
    // not reconstruct the real trajectory. These probes pin the fix.

    #[test]
    fn replay_uses_recorded_input_not_empty() {
        // A root Code node reads `n` from the run input. The successful run's
        // `output` is recomputed by `outcome_output`, which before WS4 seeded `{}`
        // (→ `n` missing → doubled 0). With the recorded input it reproduces the
        // real value, and an explicit replay reproduces it too.
        use entelechy_ir::{CodeNode, Node, NodeKind, Program};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("double", |v| {
            let n = v.data.get("n").and_then(|n| n.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({ "doubled": n * 2 }))
        });

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "calc",
                NodeKind::Code(CodeNode {
                    function: "double".into(),
                }),
            ),
        );

        let input = Value::trusted(serde_json::json!({ "n": 21 }));
        let run = e.execute(&prog, input.clone(), "run-ws4");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
        // The recomputed output reflects the real input, not an empty object.
        assert_eq!(
            run.output.as_ref().expect("output").data,
            serde_json::json!({ "doubled": 42 }),
            "outcome_output must seed the recorded input, got {:?}",
            run.output
        );

        // An explicit replay with the recorded input reproduces the same output.
        let replayed = e
            .replay(&prog, input, &run.journal)
            .expect("replay must not diverge")
            .expect("replayed output");
        assert_eq!(replayed.data, serde_json::json!({ "doubled": 42 }));
    }

    #[test]
    fn journal_records_initial_input() {
        // RK-2: the run input is persisted on the journal (so a stored journal can
        // be replayed from the real input, not an empty object).
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = engine(&model, &mut tools);
        let input = Value::trusted(serde_json::json!({ "ticket": "help", "n": 7 }));
        let run = e.execute(&demo_program(), input.clone(), "run-rec");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
        assert_eq!(
            run.journal.input, input,
            "journal must record the run input"
        );
    }

    #[test]
    fn replay_detects_input_tampering() {
        // RK-2: replaying a journal with an input other than the recorded one is a
        // typed mismatch — a faithful replay must start from the recorded input.
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = engine(&model, &mut tools);
        let run = e.execute(
            &demo_program(),
            Value::trusted(serde_json::json!({ "n": 1 })),
            "run-tamper",
        );
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);

        let tampered = e.replay(
            &demo_program(),
            Value::trusted(serde_json::json!({ "n": 999 })),
            &run.journal,
        );
        assert!(
            matches!(tampered, Err(ReplayMismatch::InputMismatch)),
            "tampered input must be rejected, got {tampered:?}"
        );
    }

    #[test]
    fn replay_reproduces_and_detects_no_divergence() {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = engine(&model, &mut tools);
        let prog = demo_program();
        let result = e.execute(&prog, Value::trusted(serde_json::json!({})), "run-1");
        assert_eq!(result.status, RunStatus::Succeeded);

        // Replay against the recorded journal: no divergence.
        let replayed = e
            .replay(
                &prog,
                Value::trusted(serde_json::json!({})),
                &result.journal,
            )
            .expect("replay should not diverge");
        assert!(replayed.is_some());
    }

    #[test]
    fn verification_failure_is_typed() {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        e.register_checker("always_false", |_| false);
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "v",
                NodeKind::Verify(VerifyNode {
                    checker: "always_false".into(),
                }),
            ),
        );
        let r = e.execute(&prog, Value::trusted(serde_json::json!({})), "run-x");
        assert!(matches!(
            r.status,
            RunStatus::Failed {
                reason: FailureReason::VerificationFailed(_),
                ..
            }
        ));
        assert!(r.output.is_none());
    }

    #[test]
    fn policy_denies_forbidden_tool_effect() {
        use entelechy_ir::{AttestationLevel, EffectClass, EffectMetadata, ToolNode};
        use entelechy_policy::{NativePolicy, PolicySnapshot};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        tools.register("refund", |_| Ok(serde_json::json!({"refunded": true})));

        // Authority forbids refund (PRD 5.4). The tool is registered, but policy
        // must deny it at the boundary before dispatch.
        let mut authority = AuthorityEnvelope::empty();
        authority.forbidden_capabilities.insert("refund".into());
        let mut catalog = HashMap::new();
        catalog.insert(
            "refund".to_string(),
            EffectMetadata {
                class: EffectClass::Irreversible,
                idempotent: false,
                reversible: false,
                dry_run_supported: false,
                read_back_supported: false,
                attestation: AttestationLevel::OperatorAttested,
                operation_key_namespace: "pay".into(),
            },
        );

        let mut e = Engine::new(&model, &mut tools).with_policy(PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: authority.clone(),
            snapshot: PolicySnapshot::default(),
            catalog,
            principal: "runtime".into(),
        });

        let prog = Program::new(
            authority,
            Node::new(
                "refund",
                NodeKind::Tool(ToolNode {
                    capability: "refund".into(),
                    args: serde_json::Value::Null,
                }),
            ),
        );
        let r = e.execute(&prog, Value::trusted(serde_json::json!({})), "run-p");
        assert!(
            matches!(
                r.status,
                RunStatus::Failed {
                    reason: FailureReason::PolicyDenied(_),
                    ..
                }
            ),
            "{:?}",
            r.status
        );
        // The denial was recorded (PRD 8.3) and the tool never ran.
        assert!(e.policy_decisions().any_denied());
    }

    #[test]
    fn model_egress_blocked_for_unapproved_provider() {
        use entelechy_ir::LlmNode;
        use entelechy_policy::{NativePolicy, PolicySnapshot, ProviderApproval};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();

        // Approve mock-small only for "internal" data, not "pii" (PRD 7.5/Q19).
        let mut approval = ProviderApproval::new();
        approval.approve("internal", "mock-small");
        let snapshot = PolicySnapshot {
            version: 1,
            provider_approval: approval,
        };

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "agent",
                NodeKind::Llm(LlmNode {
                    model: "mock-small".into(),
                    prompt_template: "{input}".into(),
                    temperature: 0.0,
                }),
            ),
        );

        // A PII-labeled input must not reach the unapproved provider.
        let mut input = Value::trusted(serde_json::json!({}));
        input.meta.confidentiality.insert("pii".into());

        let mut e = Engine::new(&model, &mut tools).with_policy(PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: AuthorityEnvelope::empty(),
            snapshot: snapshot.clone(),
            catalog: HashMap::new(),
            principal: "runtime".into(),
        });
        let r = e.execute(&prog, input.clone(), "run-egress");
        assert!(
            matches!(
                r.status,
                RunStatus::Failed {
                    reason: FailureReason::PolicyDenied(_),
                    ..
                }
            ),
            "{:?}",
            r.status
        );

        // Approve pii → mock-small and it goes through.
        let mut approval2 = ProviderApproval::new();
        approval2.approve("pii", "mock-small");
        let snapshot2 = PolicySnapshot {
            version: 2,
            provider_approval: approval2,
        };
        let mut e2 = Engine::new(&model, &mut tools).with_policy(PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: AuthorityEnvelope::empty(),
            snapshot: snapshot2,
            catalog: HashMap::new(),
            principal: "runtime".into(),
        });
        let ok = e2.execute(&prog, input, "run-egress-2");
        assert_eq!(ok.status, RunStatus::Succeeded, "{:?}", ok.status);
    }

    #[test]
    fn confidentiality_label_propagates_through_egress() {
        use entelechy_ir::LlmNode;
        use entelechy_policy::{NativePolicy, PolicySnapshot, ProviderApproval};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();

        // Approve "pii" for the first model only. The label must carry from the
        // first Llm's output into the second Llm's egress check (PRD 7.5).
        let mut approval = ProviderApproval::new();
        approval.approve("pii", "model-a");
        let snapshot = PolicySnapshot {
            version: 1,
            provider_approval: approval,
        };

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "a",
                        NodeKind::Llm(LlmNode {
                            model: "model-a".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new(
                        "b",
                        NodeKind::Llm(LlmNode {
                            model: "model-b".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                ]),
            ),
        );

        let mut input = Value::trusted(serde_json::json!({}));
        input.meta.confidentiality.insert("pii".into());

        let mut e = Engine::new(&model, &mut tools).with_policy(PolicyConfig {
            engine: Box::new(NativePolicy::new()),
            authority: AuthorityEnvelope::empty(),
            snapshot,
            catalog: HashMap::new(),
            principal: "runtime".into(),
        });
        let r = e.execute(&prog, input, "run-prop");
        // First node passes (model-a approved for pii); second fails because the
        // label propagated and model-b is not approved.
        match r.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::PolicyDenied(_),
            } => {
                assert!(
                    node_path.contains(":b"),
                    "expected failure at node b, got {node_path}"
                );
            }
            other => panic!("expected PolicyDenied at node b, got {other:?}"),
        }
    }

    #[test]
    fn counterfactual_diverges_and_reexecutes_live() {
        use entelechy_ir::{Condition, GateNode, LlmNode, ToolNode};

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        tools.register("draft_reply", |_| Ok(serde_json::json!({ "reply": "hi" })));
        let mut e = Engine::new(&model, &mut tools);
        e.register_code("route_ticket", |v| {
            let text = v.data.get("text").and_then(|t| t.as_str()).unwrap_or("");
            Ok(serde_json::json!({ "text": text, "may_reply": !text.contains("refund") }))
        });
        e.register_checker("reply_nonempty", |v| {
            v.data
                .get("reply")
                .and_then(|r| r.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        });

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "classify",
                        NodeKind::Llm(LlmNode {
                            model: "mock".into(),
                            prompt_template: "{input}".into(),
                            temperature: 0.0,
                        }),
                    ),
                    Node::new(
                        "route",
                        NodeKind::Code(CodeNode {
                            function: "route_ticket".into(),
                        }),
                    ),
                    Node::new(
                        "gate",
                        NodeKind::Gate(GateNode {
                            policy: "allow_draft".into(),
                            condition: Condition::Truthy {
                                field: "may_reply".into(),
                            },
                            requires_approval: false,
                        }),
                    ),
                    Node::new(
                        "draft",
                        NodeKind::Tool(ToolNode {
                            capability: "draft_reply".into(),
                            args: serde_json::Value::Null,
                        }),
                    ),
                    Node::new(
                        "verify",
                        NodeKind::Verify(VerifyNode {
                            checker: "reply_nonempty".into(),
                        }),
                    ),
                ]),
            ),
        );

        let run = e.execute(&prog, Value::trusted(serde_json::json!({})), "cf-run");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);

        // Counterfactual: what if `classify` had produced a non-refund ticket?
        // The pre-divergence span is seeded from the journal's recorded input.
        let cf = e.counterfactual(
            &prog,
            &run.journal,
            "root/0:classify",
            Value::tainted(serde_json::json!({ "text": "hello" })),
        );
        assert!(cf.diverged);
        assert_eq!(cf.divergence_point.as_deref(), Some("root/0:classify"));
        // The draft tool re-executes live downstream of the divergence.
        assert!(
            cf.rollouts >= 1,
            "expected >=1 rollout, got {}",
            cf.rollouts
        );
        assert!(cf.output.is_some());
    }

    #[test]
    fn model_budget_exhaustion_is_typed_terminal() {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools).with_budget(Budget {
            max_model_calls: 0,
            max_tool_calls: 10,
        });
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "llm",
                NodeKind::Llm(LlmNode {
                    model: "mock".into(),
                    prompt_template: "{input}".into(),
                    temperature: 0.0,
                }),
            ),
        );
        let r = e.execute(&prog, Value::trusted(serde_json::json!({})), "run-b");
        assert_eq!(
            r.status,
            RunStatus::BudgetExhausted {
                budget: "max_model_calls"
            }
        );
    }

    #[test]
    fn retrieval_node_reads_seeded_knowledge_and_replays_exactly() {
        use entelechy_ir::{MemoryTier, RetrievalNode};
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);

        // Seed a tiny semantic knowledge base; the matching entry carries a
        // confidentiality label that must propagate into the retrieval output.
        let mut pw = Value::trusted(serde_json::json!(
            "to reset your password, open the login page"
        ));
        pw.meta.confidentiality.insert("internal".into());
        e.seed_memory(MemoryTier::Semantic, "pw", pw);
        e.seed_memory(
            MemoryTier::Semantic,
            "refund",
            Value::trusted(serde_json::json!("refund requests take five business days")),
        );

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "fetch",
                NodeKind::Retrieval(RetrievalNode {
                    query_template: "reset password".into(),
                    tier: MemoryTier::Semantic,
                    top_k: 3,
                }),
            ),
        );

        let input = Value::trusted(serde_json::json!({}));
        let run = e.execute(&prog, input.clone(), "run-retr");
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.status);
        let out = run.output.clone().expect("retrieval output");

        // Only the matching entry is retrieved.
        let results = out
            .data
            .get("results")
            .and_then(|r| r.as_array())
            .expect("results array");
        assert_eq!(results.len(), 1, "got {results:?}");
        assert!(results[0].as_str().unwrap().contains("password"));
        // Output is tainted (external content) and carries the seeded label (IR-I9).
        assert_eq!(out.meta.taint, Taint::Tainted);
        assert!(out.meta.confidentiality.contains("internal"));
        // A deterministic in-process read records no journal effect (RK-2).
        assert!(
            run.journal.entries.is_empty(),
            "retrieval must not journal: {:?}",
            run.journal.entries
        );
        // Replay reproduces the value exactly, with no divergence.
        let replayed = e
            .replay(&prog, input, &run.journal)
            .expect("replay should not diverge")
            .expect("replayed output");
        assert_eq!(replayed.data, out.data);
    }

    #[test]
    fn human_node_fails_closed_and_never_approves() {
        // RS-2 (PRD 8.6): a human decision is never an implicit approval. Until
        // interactive human responders arrive (Phase 2+), a Human node ends in a
        // typed `Failed` terminal — it never silently succeeds or auto-approves,
        // even with a zero-second (already-expired) timeout.
        use entelechy_ir::HumanNode;
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools);
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "ask",
                NodeKind::Human(HumanNode {
                    prompt: "Approve the refund?".into(),
                    timeout_secs: 0,
                }),
            ),
        );
        let r = e.execute(&prog, Value::trusted(serde_json::json!({})), "run-human");
        assert!(
            matches!(r.status, RunStatus::Failed { .. }),
            "human node must fail closed, got {:?}",
            r.status
        );
        assert_ne!(r.status, RunStatus::Succeeded);
    }
}
