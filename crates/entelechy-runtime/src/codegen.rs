//! Compiled execution backend and the codegen conformance/promotion gate
//! (PRD 21 Phase 4, Q10, risk "runtime/codegen divergence").
//!
//! Codegen is optional: it becomes a production runtime only when it passes the
//! shared conformance suite, replays >=1,000 journals identically, shows no
//! holdout regression, and cuts p95 latency or cost by >=30% on >=3 objectives
//! (Q10); otherwise the interpreter stays the production runtime.
//!
//! This module provides the substrate for that decision without a full
//! source-emitting compiler: [`CompiledPlan::compile`] lowers an IR program's
//! deterministic structural core to a compiled step tree (a real compilation step
//! distinct from the tree-walking interpreter — sequencing is flattened and control
//! flow is lowered to nested step lists, while each failable leaf records the
//! interpreter's node path so a *failure* is observably identical too),
//! [`Engine::run_compiled`] executes it — enforcing the *same* effect gates as the
//! interpreter: egress authorization and the model-call budget before each inference
//! (PRD 7.5, 8.3), policy + delegated-authority authorization and the tool-call budget
//! before each consequential effect (WS2/WS3, PRD 5.4, 8.3), and `Delegate` authority
//! narrowing. [`observable_equivalent`] checks interpreter-vs-codegen conformance, and
//! [`CodegenEvidence`] encodes the Q10 promotion criteria. A richer backend (e.g.
//! emitting Rust) implements the same lowering and must satisfy the same conformance
//! gate. The compiled backend therefore matches the interpreter not only on the happy
//! path but on every halt it can reach — egress-denied, policy-denied,
//! authority-denied, gate-blocked, reconciliation-required and budget-exhausted; the
//! shapes it defers (below) remain the only interpreter-only envelope.
//!
//! Supported shapes: the `Llm`/`Code`/`Verify`/`Mem` leaves, `Retrieval`, the
//! deterministic control flow `Seq`/`Branch`/`Loop`/`Map`, the `Gate` policy check,
//! the consequential `Tool` effect and the authority-narrowing `Delegate`. Only `Par`
//! and `Human` return [`CompileError::Unsupported`] and run on the interpreter, the
//! reference executor for the conformance gate — `Par` carries bounded-parallelism
//! semantics the backend keeps deferred so a future *truly* concurrent interpreter
//! cannot diverge from a sequential lowering, and `Human` carries interactive-input
//! semantics (an interactive responder, Phase 2+).

use serde::{Deserialize, Serialize};

use entelechy_ir::{
    AuthorityEnvelope, Condition, MemOp, MemoryTier, Node, NodeKind, Program, Value, ValueMeta,
};

use crate::journal::Journal;
use crate::{
    egress_output, eval_condition, get_path, merge_args, Engine, FailureReason, RunResult,
    RunStatus,
};

/// A lowered instruction (the "compiled" form of a supported IR node). Control
/// flow nests further steps, so a plan is a step *tree*, not a flat list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompiledStep {
    /// Model inference.
    Llm {
        /// This step's interpreter node path, relative to the enclosing dynamic
        /// scope (the run root, or a `Loop`/`Map` body iteration). The executor
        /// joins it with the runtime base to report the interpreter's exact
        /// `node_path` on failure, so a failing run is observably identical to the
        /// interpreter (Q10 conformance on the failure path).
        path: String,
        /// Model identity.
        model: String,
        /// Prompt template.
        prompt_template: String,
        /// Sampling temperature.
        temperature: f64,
    },
    /// Deterministic code function.
    Code {
        /// This step's interpreter node path relative to the enclosing dynamic
        /// scope (as for the `Llm` step's `path`).
        path: String,
        /// Registered function name.
        function: String,
    },
    /// Verification checker.
    Verify {
        /// This step's interpreter node path relative to the enclosing dynamic
        /// scope (as for the `Llm` step's `path`).
        path: String,
        /// Checker name.
        checker: String,
    },
    /// Read from a memory tier.
    MemRead {
        /// Tier.
        tier: MemoryTier,
        /// Key.
        key: String,
    },
    /// Write the current value to a memory tier.
    MemWrite {
        /// Tier.
        tier: MemoryTier,
        /// Key.
        key: String,
    },
    /// Retrieve the `top_k` most relevant entries of a tier for a templated query
    /// (PRD 12). The result enters as tainted, joined with the input's and every
    /// hit's labels (IR-I9) — identical to the interpreter's `Retrieval` arm.
    Retrieve {
        /// Tier searched.
        tier: MemoryTier,
        /// Query template (`{input}` replaced with the current value's payload).
        query_template: String,
        /// Maximum number of entries returned.
        top_k: u32,
    },
    /// Typed conditional: run `then` when the condition holds, else `els`.
    Branch {
        /// Condition evaluated against the current value.
        cond: Condition,
        /// Steps run when the condition holds.
        then: Vec<CompiledStep>,
        /// Steps run otherwise.
        els: Vec<CompiledStep>,
    },
    /// Bounded iteration: run `body` until `until` holds or `max_iters` is reached
    /// (IR-I4). `until` is checked before each iteration, as in the interpreter.
    Loop {
        /// The loop node's own interpreter path, relative to the enclosing dynamic
        /// scope (as for the `Llm` step's `path`). Joined with `body_id` and the
        /// runtime iteration index to form each iteration's path base
        /// (`<loop>/iter<i>:<body_id>`), so body-step failures report the
        /// interpreter's exact `node_path`.
        path: String,
        /// The loop body node's id, the `<body_id>` segment of each iteration's base.
        body_id: String,
        /// Steps run each iteration.
        body: Vec<CompiledStep>,
        /// Hard upper bound on iterations (IR-I4).
        max_iters: u32,
        /// Stop early when this becomes true.
        until: Condition,
    },
    /// Fan-out: apply `body` to each element of the collection at dot-path `over`.
    /// Each element inherits the collection's labels and the result joins them all
    /// (IR-I9) — identical to the interpreter's `Map` arm.
    Map {
        /// The map node's own interpreter path, relative to the enclosing dynamic
        /// scope (as for the `Llm` step's `path`). Joined with `body_id` and the
        /// runtime element index to form each element's path base
        /// (`<map>/map<i>:<body_id>`).
        path: String,
        /// The map body node's id, the `<body_id>` segment of each element's base.
        body_id: String,
        /// Dot-path to the input collection field.
        over: String,
        /// Steps applied to each element.
        body: Vec<CompiledStep>,
    },
    /// Policy gate (PRD 7.3): open when the condition holds and no approval is
    /// required, declassifying the value to `Trusted` for integrity (IR-I3); else a
    /// typed `GateBlocked` halt. Mirrors the interpreter's `Gate` arm — the Phase-0
    /// runtime cannot discharge an approval obligation inline, so a gate requiring
    /// approval blocks.
    Gate {
        /// This gate's interpreter node path, relative to the enclosing dynamic scope
        /// (as for the `Llm` step's `path`).
        path: String,
        /// The policy name reported on a `GateBlocked` halt.
        policy: String,
        /// Condition evaluated against the current value.
        condition: Condition,
        /// Whether the gate requires an approval the Phase-0 runtime cannot discharge
        /// inline (so it blocks).
        requires_approval: bool,
    },
    /// Consequential tool effect (PRD 7.6, 8.3). Authorized at its boundary against
    /// the active (possibly `Delegate`-narrowed) authority envelope and the policy
    /// layer (WS2), charged the tool-call budget (WS3), then dispatched through the
    /// tool gateway — exactly as the interpreter's `Execute` `Tool` arm. An
    /// unacknowledged dispatch forces `ReconciliationRequired`.
    Tool {
        /// This tool node's interpreter path, relative to the enclosing dynamic scope
        /// (as for the `Llm` step's `path`).
        path: String,
        /// The capability invoked (authorized before dispatch).
        capability: String,
        /// Static arguments, merged with the current value's payload (static wins),
        /// identically to the interpreter (`merge_args`).
        args: serde_json::Value,
    },
    /// Authority-narrowing delegation (A2 / IR-I6, PRD 5.4). A delegate that *widens*
    /// the active envelope halts (`AuthorityDenied`) — defense-in-depth beyond the
    /// static IR-I6 check; otherwise the narrowed envelope is in force for the body,
    /// so every `Tool` inside is authorized against it. A `Delegate` opens no new path
    /// scope (its body steps carry the static `/delegate:<id>` segment), so the body
    /// runs under the same runtime base.
    Delegate {
        /// The delegate node's own interpreter path, relative to the enclosing dynamic
        /// scope — where a widening halt is reported (as for the `Llm` step's `path`).
        path: String,
        /// The narrowed authority envelope pushed for the body.
        authority: AuthorityEnvelope,
        /// Steps run under the narrowed envelope.
        body: Vec<CompiledStep>,
    },
}

/// A compiled program: a step tree plus the authority envelope (PRD Q10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledPlan {
    /// The authority envelope (carried for parity with the interpreter).
    pub authority: AuthorityEnvelope,
    /// The lowered steps, in execution order.
    pub steps: Vec<CompiledStep>,
}

/// Why a program cannot be compiled by this backend (the interpreter remains the
/// fallback — Q10).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CompileError {
    /// A node kind is not supported by the compiled backend.
    #[error("codegen does not support node kind '{0}' (interpreter remains the runtime)")]
    Unsupported(&'static str),
}

impl CompiledPlan {
    /// Lower an IR program's deterministic structural core into a compiled plan.
    /// Sequencing is flattened; branches, bounded loops, map fan-out and retrieval
    /// lower recursively over the `Llm`/`Code`/`Verify`/`Mem` leaves. Nodes the
    /// compiled backend defers to the interpreter (`Par`, `Delegate`, `Tool`,
    /// `Gate`, `Human`) return `Unsupported`, so the interpreter stays the runtime
    /// for them (Q10).
    pub fn compile(program: &Program) -> Result<CompiledPlan, CompileError> {
        Ok(CompiledPlan {
            authority: program.authority.clone(),
            // The run root is executed by the interpreter at path "root"; the root
            // node is itself the top dynamic base, so its path relative to that base
            // is empty. The executor threads "root" as the base at run time.
            steps: lower_node(&program.root, "")?,
        })
    }
}

/// Lower one IR node into its compiled steps. A `Seq` flattens into its children's
/// steps (a `Seq` of `Seq`s collapses); every other supported node lowers to a
/// single step (whose bodies lower recursively).
///
/// `rel` is this node's path **relative to the enclosing dynamic scope** — the run
/// root, or a `Loop`/`Map` body iteration whose base carries a runtime index. Static
/// structure (`Seq`, `Branch`) extends `rel` at compile time, so every failable leaf
/// records the exact segment chain the interpreter would build; `Loop`/`Map` open a
/// fresh scope whose body is lowered relative to an empty base (the body *is* the new
/// base) and whose runtime index the executor supplies (Q10 failure-path parity).
fn lower_node(node: &Node, rel: &str) -> Result<Vec<CompiledStep>, CompileError> {
    Ok(match &node.kind {
        NodeKind::Seq(children) => lower_children(children, rel)?,
        NodeKind::Llm(l) => vec![CompiledStep::Llm {
            path: rel.to_string(),
            model: l.model.clone(),
            prompt_template: l.prompt_template.clone(),
            temperature: l.temperature,
        }],
        NodeKind::Code(c) => vec![CompiledStep::Code {
            path: rel.to_string(),
            function: c.function.clone(),
        }],
        NodeKind::Verify(v) => vec![CompiledStep::Verify {
            path: rel.to_string(),
            checker: v.checker.clone(),
        }],
        NodeKind::Mem(m) => vec![match m.op {
            MemOp::Read => CompiledStep::MemRead {
                tier: m.tier,
                key: m.key.clone(),
            },
            MemOp::Write => CompiledStep::MemWrite {
                tier: m.tier,
                key: m.key.clone(),
            },
        }],
        NodeKind::Retrieval(r) => vec![CompiledStep::Retrieve {
            tier: r.tier,
            query_template: r.query_template.clone(),
            top_k: r.top_k,
        }],
        // A `Branch` does not open a new dynamic scope (its tag and arm ids are
        // static), so each arm is lowered relative to the *same* base, extending
        // `rel` with the interpreter's `<tag>:<arm.id>` segment.
        NodeKind::Branch { cond, then, els } => vec![CompiledStep::Branch {
            cond: cond.clone(),
            then: lower_node(then, &format!("{rel}/then:{}", then.id))?,
            els: lower_node(els, &format!("{rel}/else:{}", els.id))?,
        }],
        NodeKind::Loop {
            body,
            max_iters,
            until,
        } => vec![CompiledStep::Loop {
            path: rel.to_string(),
            body_id: body.id.clone(),
            body: lower_node(body, "")?,
            max_iters: *max_iters,
            until: until.clone(),
        }],
        NodeKind::Map { over, body } => vec![CompiledStep::Map {
            path: rel.to_string(),
            body_id: body.id.clone(),
            over: over.clone(),
            body: lower_node(body, "")?,
        }],
        NodeKind::Gate(g) => vec![CompiledStep::Gate {
            path: rel.to_string(),
            policy: g.policy.clone(),
            condition: g.condition.clone(),
            requires_approval: g.requires_approval,
        }],
        NodeKind::Tool(t) => vec![CompiledStep::Tool {
            path: rel.to_string(),
            capability: t.capability.clone(),
            args: t.args.clone(),
        }],
        // A `Delegate` opens no new dynamic scope for paths (its body id is static),
        // so the body is lowered relative to the same base, extending `rel` with the
        // interpreter's `/delegate:<body.id>` segment.
        NodeKind::Delegate { authority, body } => vec![CompiledStep::Delegate {
            path: rel.to_string(),
            authority: authority.clone(),
            body: lower_node(body, &format!("{rel}/delegate:{}", body.id))?,
        }],
        // Deferred to the interpreter (the conformance reference executor): `Par`
        // keeps bounded-parallelism semantics deferred so a sequential lowering cannot
        // diverge from a future truly-concurrent interpreter, and `Human` carries
        // interactive-input semantics (Phase 2+) (Q10).
        NodeKind::Par(_) => return Err(CompileError::Unsupported("par")),
        NodeKind::Human(_) => return Err(CompileError::Unsupported("human")),
    })
}

/// Lower an ordered list of children, flattening their steps in order. Each child
/// `i` extends the parent's relative path with the interpreter's `/<i>:<id>` segment.
fn lower_children(children: &[Node], parent_rel: &str) -> Result<Vec<CompiledStep>, CompileError> {
    let mut steps = Vec::new();
    for (i, child) in children.iter().enumerate() {
        let child_rel = format!("{parent_rel}/{i}:{}", child.id);
        steps.extend(lower_node(child, &child_rel)?);
    }
    Ok(steps)
}

/// Run-global effect-call budget counters for the compiled executor, mirroring the
/// interpreter's `exec::Counters` (model + tool calls). Charged per run exactly as
/// the interpreter charges them, so a compiled run halts on the same budget
/// (WS3, PRD 8.3).
#[derive(Default)]
struct RunCounters {
    model_calls: u32,
    tool_calls: u32,
}

impl Engine<'_> {
    /// Execute a [`CompiledPlan`] (the codegen backend). Produces the same
    /// observable state as the interpreter for the supported subset — including the
    /// failure path (terminal status node path + output) — verified by
    /// [`observable_equivalent`] against `execute` (Q10 conformance). The run root is
    /// executed at path `"root"` (the base each leaf's relative path is joined onto),
    /// and the model/tool-call budgets are charged per run exactly as the interpreter
    /// does.
    pub fn run_compiled(&mut self, plan: &CompiledPlan, input: Value, run_id: &str) -> RunResult {
        // Seed the delegation authority stack from the plan authority (A1/WS2),
        // exactly as `execute` seeds it from `program.authority`, so a compiled `Tool`
        // inside a `Delegate` is authorized against the narrowed envelope identically
        // to the interpreter.
        self.authority_stack = vec![plan.authority.clone()];
        let mut counters = RunCounters::default();
        match self.run_steps(&plan.steps, input, "root", run_id, &mut counters) {
            Ok(value) => RunResult {
                status: RunStatus::Succeeded,
                output: Some(value),
                journal: Journal::new(run_id),
            },
            // The recursive executor returns the terminal `RunStatus` directly — a
            // `Failed { node_path, reason }` carrying the interpreter's node path, or a
            // `BudgetExhausted`. The compiled backend journals nothing, so a fresh
            // journal is attached here. `RunStatus` is small, so the `Err` variant
            // stays well under `clippy::result_large_err`.
            Err(status) => RunResult {
                status,
                output: None,
                journal: Journal::new(run_id),
            },
        }
    }

    /// Run a step list in order, threading the value. `base` is the runtime path of
    /// the enclosing dynamic scope (`"root"`, or a `Loop`/`Map` body iteration), onto
    /// which each step's relative `path` is joined to form the interpreter's node
    /// path; `run_id` keys each `Tool` effect's operation key (as the interpreter's
    /// journal run id does); `counters` is the run-global model/tool-call budget. A
    /// terminal halt is returned as `Err(RunStatus)` and short-circuits the remaining
    /// steps.
    fn run_steps(
        &mut self,
        steps: &[CompiledStep],
        mut value: Value,
        base: &str,
        run_id: &str,
        counters: &mut RunCounters,
    ) -> Result<Value, RunStatus> {
        for step in steps {
            value = self.run_step(step, value, base, run_id, counters)?;
        }
        Ok(value)
    }

    /// Execute one compiled step. Each arm mirrors the corresponding interpreter arm
    /// in `exec` so the observable result (terminal status incl. node path + output
    /// incl. labels) matches (Q10).
    fn run_step(
        &mut self,
        step: &CompiledStep,
        value: Value,
        base: &str,
        run_id: &str,
        counters: &mut RunCounters,
    ) -> Result<Value, RunStatus> {
        match step {
            CompiledStep::Llm {
                path,
                model,
                prompt_template,
                temperature,
            } => {
                let node_path = format!("{base}{path}");
                // Egress authorization (IR-I9) then the model-call budget, before
                // dispatch — mirroring the interpreter's `guard_model` (PRD 7.5, 8.3).
                // A model (inference) call is not a consequential world-effect, so this
                // is the only authority/budget gate a compiled leaf can reach (`Tool`
                // is deferred to the interpreter) — the gate that widens the
                // conformance envelope to egress/budget halts. `authorize_model` is a
                // no-op without policy or confidentiality labels, so the supported
                // happy path is unchanged.
                if let Err(reason) = self.authorize_model(model, &value.meta.confidentiality) {
                    return Err(RunStatus::Failed { node_path, reason });
                }
                counters.model_calls += 1;
                if counters.model_calls > self.budget.max_model_calls {
                    return Err(RunStatus::BudgetExhausted {
                        budget: "max_model_calls",
                    });
                }
                let prompt = prompt_template.replace("{input}", &value.data.to_string());
                let request = entelechy_gateway::ModelRequest {
                    model: model.clone(),
                    prompt,
                    temperature: *temperature,
                };
                match self.model.infer(&request) {
                    Ok(response) => Ok(egress_output(
                        serde_json::json!({ "text": response.text }),
                        &value.meta,
                    )),
                    Err(e) => Err(RunStatus::Failed {
                        node_path,
                        reason: FailureReason::ModelError(e.to_string()),
                    }),
                }
            }
            CompiledStep::Code { path, function } => match self.code.get(function) {
                Some(f) => match f(&value) {
                    Ok(data) => Ok(Value {
                        data,
                        meta: value.meta,
                    }),
                    Err(e) => Err(RunStatus::Failed {
                        node_path: format!("{base}{path}"),
                        reason: FailureReason::CodeError(e),
                    }),
                },
                None => Err(RunStatus::Failed {
                    node_path: format!("{base}{path}"),
                    reason: FailureReason::CodeError(format!("unknown function '{function}'")),
                }),
            },
            CompiledStep::Verify { path, checker } => match self.checkers.get(checker) {
                Some(c) => {
                    if c(&value) {
                        let mut out = value;
                        out.meta.verification = entelechy_ir::VerificationState::Verified;
                        Ok(out)
                    } else {
                        Err(RunStatus::Failed {
                            node_path: format!("{base}{path}"),
                            reason: FailureReason::VerificationFailed(checker.clone()),
                        })
                    }
                }
                None => Err(RunStatus::Failed {
                    node_path: format!("{base}{path}"),
                    reason: FailureReason::VerificationFailed(format!(
                        "unknown checker '{checker}'"
                    )),
                }),
            },
            CompiledStep::MemRead { tier, key } => Ok(self
                .memory
                .read(*tier, key)
                .cloned()
                .unwrap_or_else(|| Value::trusted(serde_json::Value::Null))),
            CompiledStep::MemWrite { tier, key } => {
                self.memory.write(*tier, key.clone(), value.clone());
                Ok(value)
            }
            CompiledStep::Retrieve {
                tier,
                query_template,
                top_k,
            } => {
                let query = query_template.replace("{input}", &value.data.to_string());
                let hits = self.memory.retrieve(*tier, &query, *top_k);
                let mut meta = ValueMeta::tainted().join(&value.meta);
                let mut results = Vec::with_capacity(hits.len());
                for h in &hits {
                    meta = meta.join(&h.meta);
                    results.push(h.data.clone());
                }
                Ok(Value {
                    data: serde_json::json!({ "query": query, "results": results }),
                    meta,
                })
            }
            CompiledStep::Branch { cond, then, els } => {
                // A branch opens no new dynamic scope: the chosen arm runs against the
                // same `base` (its steps already carry the `<tag>:<arm.id>` segment).
                let branch = if eval_condition(cond, &value) {
                    then
                } else {
                    els
                };
                self.run_steps(branch, value, base, run_id, counters)
            }
            CompiledStep::Loop {
                path,
                body_id,
                body,
                max_iters,
                until,
            } => {
                let loop_path = format!("{base}{path}");
                let mut cur = value;
                for i in 0..*max_iters {
                    if eval_condition(until, &cur) {
                        break;
                    }
                    // Each iteration is a fresh dynamic scope whose base carries the
                    // runtime index, exactly as the interpreter builds the path.
                    let body_base = format!("{loop_path}/iter{i}:{body_id}");
                    cur = self.run_steps(body, cur, &body_base, run_id, counters)?;
                }
                Ok(cur)
            }
            CompiledStep::Map {
                path,
                body_id,
                over,
                body,
            } => {
                let map_path = format!("{base}{path}");
                let items = get_path(&value.data, over)
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
                let mut out = Vec::with_capacity(items.len());
                let mut meta = value.meta.clone();
                for (i, item) in items.into_iter().enumerate() {
                    let body_base = format!("{map_path}/map{i}:{body_id}");
                    let item_value = Value {
                        data: item,
                        meta: value.meta.clone(),
                    };
                    let r = self.run_steps(body, item_value, &body_base, run_id, counters)?;
                    meta = meta.join(&r.meta);
                    out.push(r.data);
                }
                Ok(Value {
                    data: serde_json::Value::Array(out),
                    meta,
                })
            }
            CompiledStep::Gate {
                path,
                policy,
                condition,
                requires_approval,
            } => {
                // Mirror the interpreter's `Gate` arm: open iff the condition holds
                // and no approval is required (the Phase-0 runtime cannot discharge an
                // approval obligation inline). An opened gate declassifies the value to
                // `Trusted` for integrity (IR-I3); a closed gate is a typed
                // `GateBlocked` halt. The interpreter also journals a `PolicyDecision`
                // here — invisible to `observable_equivalent` (status + output), so the
                // compiled backend journals nothing and still conforms.
                if eval_condition(condition, &value) && !*requires_approval {
                    let mut out = value;
                    out.meta.taint = entelechy_ir::Taint::Trusted;
                    Ok(out)
                } else {
                    Err(RunStatus::Failed {
                        node_path: format!("{base}{path}"),
                        reason: FailureReason::GateBlocked(policy.clone()),
                    })
                }
            }
            CompiledStep::Tool {
                path,
                capability,
                args,
            } => {
                let node_path = format!("{base}{path}");
                // Authorize the effect at its boundary against the active (possibly
                // `Delegate`-narrowed) authority envelope and the policy layer (WS2),
                // then charge the tool-call budget (WS3), before dispatch — mirroring
                // the interpreter's `guard_tool` (PRD 8.3). `authorize_tool` is shared
                // with the interpreter, so a denial carries the identical reason.
                if let Err(reason) = self.authorize_tool(capability, value.meta.taint) {
                    return Err(RunStatus::Failed { node_path, reason });
                }
                counters.tool_calls += 1;
                if counters.tool_calls > self.budget.max_tool_calls {
                    return Err(RunStatus::BudgetExhausted {
                        budget: "max_tool_calls",
                    });
                }
                // Build the identical `ToolCall` the interpreter dispatches (same
                // operation key + merged args), so a deterministic gateway returns the
                // same response and the egress output matches.
                let call = entelechy_gateway::ToolCall {
                    capability: capability.clone(),
                    args: merge_args(args, &value.data),
                    operation_key: format!("{run_id}/{node_path}/attempt0"),
                };
                match self.tools.call(&call) {
                    Ok(response) => Ok(egress_output(response, &value.meta)),
                    // An unacknowledged dispatch forces reconciliation, exactly as the
                    // interpreter's `Execute` `Tool` arm: it leaves a dangling Unknown
                    // intent that derives `ReconciliationRequired`. The compiled backend
                    // journals nothing, so it returns that terminal directly.
                    Err(_) => Err(RunStatus::ReconciliationRequired),
                }
            }
            CompiledStep::Delegate {
                path,
                authority,
                body,
            } => {
                // Runtime authority narrowing (A2 / IR-I6, WS2), mirroring the
                // interpreter's `Delegate` arm: a delegate that *widens* the active
                // envelope halts (defense-in-depth beyond the static IR-I6 check);
                // otherwise the narrowed envelope is pushed for the body — so every
                // `Tool` inside is authorized against it by `authorize_tool` — and
                // popped afterwards (even on a halt, so the stack stays balanced). The
                // body runs under the same runtime `base` (a `Delegate` opens no new
                // path scope).
                let widens = self
                    .authority_stack
                    .last()
                    .map(|active| !authority.is_subset_of(active))
                    .unwrap_or(false);
                if widens {
                    return Err(RunStatus::Failed {
                        node_path: format!("{base}{path}"),
                        reason: FailureReason::AuthorityDenied(
                            "delegated authority widens the active envelope (A2/IR-I6)".into(),
                        ),
                    });
                }
                self.authority_stack.push(authority.clone());
                let result = self.run_steps(body, value, base, run_id, counters);
                self.authority_stack.pop();
                result
            }
        }
    }
}

/// Whether two runs have the same observable state (terminal status + output).
/// This is the interpreter-vs-codegen conformance check (PRD Q10, risk
/// "runtime/codegen divergence").
pub fn observable_equivalent(a: &RunResult, b: &RunResult) -> bool {
    a.status == b.status && a.output == b.output
}

/// Evidence for the Q10 codegen-promotion decision.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodegenEvidence {
    /// Passed 100% of the shared conformance suite.
    pub conformance_pass: bool,
    /// Number of recorded journals replayed identically (Q10 requires >= 1000).
    pub identical_replays: u32,
    /// Whether a holdout regression was observed (must be false).
    pub holdout_regression: bool,
    /// Best p95-latency or cost improvement as a fraction (Q10 requires >= 0.30).
    pub p95_or_cost_improvement: f64,
    /// Number of objectives where the improvement held (Q10 requires >= 3).
    pub objectives_improved: u32,
}

impl CodegenEvidence {
    /// Whether codegen may become a production runtime (PRD Q10). Otherwise the
    /// interpreter stays the production runtime.
    pub fn codegen_eligible(&self) -> bool {
        self.conformance_pass
            && self.identical_replays >= 1_000
            && !self.holdout_regression
            && self.p95_or_cost_improvement >= 0.30
            && self.objectives_improved >= 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_gateway::{MockModel, NativeToolGateway};
    // `Condition`, `MemoryTier`, `Node`, `NodeKind`, `Program`, `AuthorityEnvelope`
    // and `Value` come in via `use super::*`; only the node payloads not imported
    // at module scope are named here.
    use entelechy_ir::{CodeNode, GateNode, LlmNode, RetrievalNode, ToolNode, VerifyNode};

    fn linear_program() -> Program {
        Program::new(
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

    /// Register every code function / checker the conformance programs use, on
    /// both the interpreter and codegen engines (so the only difference under test
    /// is the execution backend).
    fn register(e: &mut Engine) {
        e.register_code("tag_ok", |v| {
            let mut obj = v.data.clone();
            if let Some(o) = obj.as_object_mut() {
                o.insert("ok".into(), serde_json::json!(true));
            }
            Ok(obj)
        });
        e.register_checker("has_text", |v| v.data.get("text").is_some());
        // Always fails (failure-path conformance tests).
        e.register_checker("always_false", |_| false);
        // Increment `n` (loop conformance test; also seeds n=0 from `{}`).
        e.register_code("incr", |v| {
            let n = v.data.get("n").and_then(|x| x.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({ "n": n + 1 }))
        });
        // Wrap the current value under `item` (map conformance test).
        e.register_code("wrap", |v| Ok(serde_json::json!({ "item": v.data })));
        // Seed a collection (map conformance test).
        e.register_code("seed_xs", |_| Ok(serde_json::json!({ "xs": [1, 2, 3] })));
        // Record which branch ran (branch conformance test).
        e.register_code("mark_then", |_| Ok(serde_json::json!({ "path": "then" })));
        e.register_code("mark_else", |_| Ok(serde_json::json!({ "path": "else" })));
    }

    /// Register the deterministic tools the `Tool`-conformance programs dispatch, on
    /// the gateway both backends share — so a compiled `Tool` and the interpreter's
    /// `Tool` call the identical, deterministic capability.
    fn register_tools(tools: &mut NativeToolGateway) {
        tools.register("draft_reply", |_| Ok(serde_json::json!({ "reply": "hi" })));
        tools.register("publish", |_| Ok(serde_json::json!({ "published": true })));
    }

    /// Run a program on a fresh interpreter engine.
    fn run_interp(prog: &Program) -> RunResult {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        register_tools(&mut tools);
        let mut e = Engine::new(&model, &mut tools);
        register(&mut e);
        e.execute(prog, Value::trusted(serde_json::json!({})), "r")
    }

    /// Compile and run a program on a fresh codegen engine.
    fn run_codegen(prog: &Program) -> RunResult {
        let plan = CompiledPlan::compile(prog).expect("compilable");
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        register_tools(&mut tools);
        let mut e = Engine::new(&model, &mut tools);
        register(&mut e);
        e.run_compiled(&plan, Value::trusted(serde_json::json!({})), "r")
    }

    /// Interpreter and codegen must agree (Q10). Asserts success too, so a shape
    /// that silently fails in both can't pass the conformance check vacuously.
    fn assert_conformant(prog: &Program) {
        let interp = run_interp(prog);
        let codegen = run_codegen(prog);
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
        assert_eq!(interp.status, RunStatus::Succeeded, "expected success");
    }

    /// Run a program on both backends under an optional budget and an optional
    /// freshly-built policy (each engine gets its own, so a `Box<dyn PolicyEngine>`
    /// is produced per backend), returning both results. Used for conformance on
    /// *halts* (budget/egress/failure) where `assert_conformant`'s success
    /// requirement does not apply — the widened envelope this unit adds.
    fn run_both(
        prog: &Program,
        input: Value,
        budget: Option<crate::Budget>,
        mk_policy: impl Fn() -> Option<crate::PolicyConfig>,
    ) -> (RunResult, RunResult) {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        register_tools(&mut tools);
        let mut interp_engine = Engine::new(&model, &mut tools);
        if let Some(b) = budget {
            interp_engine = interp_engine.with_budget(b);
        }
        if let Some(p) = mk_policy() {
            interp_engine = interp_engine.with_policy(p);
        }
        register(&mut interp_engine);
        let interp = interp_engine.execute(prog, input.clone(), "r");

        let plan = CompiledPlan::compile(prog).expect("compilable");
        let model2 = MockModel::new();
        let mut tools2 = NativeToolGateway::new();
        register_tools(&mut tools2);
        let mut codegen_engine = Engine::new(&model2, &mut tools2);
        if let Some(b) = budget {
            codegen_engine = codegen_engine.with_budget(b);
        }
        if let Some(p) = mk_policy() {
            codegen_engine = codegen_engine.with_policy(p);
        }
        register(&mut codegen_engine);
        let codegen = codegen_engine.run_compiled(&plan, input, "r");

        (interp, codegen)
    }

    #[test]
    fn codegen_matches_the_interpreter() {
        let prog = linear_program();
        let plan = CompiledPlan::compile(&prog).unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert_conformant(&prog);
    }

    #[test]
    fn nested_seq_flattens_and_conforms() {
        // A Seq nested inside the root Seq flattens to the same step list.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "inner",
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
                                "tag",
                                NodeKind::Code(CodeNode {
                                    function: "tag_ok".into(),
                                }),
                            ),
                        ]),
                    ),
                    Node::new(
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "has_text".into(),
                        }),
                    ),
                ]),
            ),
        );
        let plan = CompiledPlan::compile(&prog).unwrap();
        assert_eq!(plan.steps.len(), 3, "nested Seq should flatten");
        assert_conformant(&prog);
    }

    fn branch_program(cond: Condition) -> Program {
        Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Branch {
                    cond,
                    then: Box::new(Node::new(
                        "t",
                        NodeKind::Code(CodeNode {
                            function: "mark_then".into(),
                        }),
                    )),
                    els: Box::new(Node::new(
                        "e",
                        NodeKind::Code(CodeNode {
                            function: "mark_else".into(),
                        }),
                    )),
                },
            ),
        )
    }

    #[test]
    fn branch_both_arms_conform() {
        // Condition holds -> `then`; the input `{}` has no `flag`, so Truthy is
        // false -> `els`. Run both outcomes.
        assert_conformant(&branch_program(Condition::Always));
        assert_conformant(&branch_program(Condition::Truthy {
            field: "flag".into(),
        }));
    }

    #[test]
    fn loop_conforms() {
        // Seed n=0->1, then increment until n==3; the bound (10) is never hit.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "seed",
                        NodeKind::Code(CodeNode {
                            function: "incr".into(),
                        }),
                    ),
                    Node::new(
                        "loop",
                        NodeKind::Loop {
                            body: Box::new(Node::new(
                                "step",
                                NodeKind::Code(CodeNode {
                                    function: "incr".into(),
                                }),
                            )),
                            max_iters: 10,
                            until: Condition::Equals {
                                field: "n".into(),
                                value: serde_json::json!(3),
                            },
                        },
                    ),
                ]),
            ),
        );
        assert_conformant(&prog);
    }

    #[test]
    fn map_conforms() {
        // Fan-out `wrap` over a 3-element collection; results and joined labels
        // must match the interpreter.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![
                    Node::new(
                        "seed",
                        NodeKind::Code(CodeNode {
                            function: "seed_xs".into(),
                        }),
                    ),
                    Node::new(
                        "fan",
                        NodeKind::Map {
                            over: "xs".into(),
                            body: Box::new(Node::new(
                                "w",
                                NodeKind::Code(CodeNode {
                                    function: "wrap".into(),
                                }),
                            )),
                        },
                    ),
                ]),
            ),
        );
        assert_conformant(&prog);
        // The fan-out produced three wrapped elements.
        let out = run_interp(&prog).output.unwrap();
        assert_eq!(out.data.as_array().map(|a| a.len()), Some(3));
    }

    #[test]
    fn retrieval_conforms_on_empty_memory() {
        // Retrieval over an empty tier returns no hits; both runtimes build the
        // same tainted `{query, results: []}` value.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Retrieval(RetrievalNode {
                    query_template: "about {input}".into(),
                    tier: MemoryTier::Semantic,
                    top_k: 3,
                }),
            ),
        );
        assert_conformant(&prog);
    }

    #[test]
    fn unsupported_shapes_are_refused() {
        // Only `Par` (deferred so a sequential lowering cannot diverge from a future
        // truly-concurrent interpreter) and `Human` (interactive, Phase 2+) remain
        // interpreter-only; `Gate`/`Tool`/`Delegate` are now lowered.
        use entelechy_ir::HumanNode;
        let par = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "p",
                NodeKind::Par(vec![Node::new(
                    "x",
                    NodeKind::Code(CodeNode {
                        function: "tag_ok".into(),
                    }),
                )]),
            ),
        );
        assert_eq!(
            CompiledPlan::compile(&par),
            Err(CompileError::Unsupported("par"))
        );
        let human = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "ask",
                NodeKind::Human(HumanNode {
                    prompt: "?".into(),
                    timeout_secs: 0,
                }),
            ),
        );
        assert_eq!(
            CompiledPlan::compile(&human),
            Err(CompileError::Unsupported("human"))
        );
    }

    #[test]
    fn q10_promotion_gate() {
        let ok = CodegenEvidence {
            conformance_pass: true,
            identical_replays: 1_000,
            holdout_regression: false,
            p95_or_cost_improvement: 0.30,
            objectives_improved: 3,
        };
        assert!(ok.codegen_eligible());

        // Any single criterion failing keeps the interpreter as the runtime.
        assert!(!CodegenEvidence {
            identical_replays: 999,
            ..ok
        }
        .codegen_eligible());
        assert!(!CodegenEvidence {
            p95_or_cost_improvement: 0.29,
            ..ok
        }
        .codegen_eligible());
        assert!(!CodegenEvidence {
            objectives_improved: 2,
            ..ok
        }
        .codegen_eligible());
        assert!(!CodegenEvidence {
            holdout_regression: true,
            ..ok
        }
        .codegen_eligible());
        assert!(!CodegenEvidence {
            conformance_pass: false,
            ..ok
        }
        .codegen_eligible());
    }

    // ===== Conformance on halts: the widened envelope (budget / egress / failure) =====
    //
    // Before this unit the compiled backend ignored the model-call budget and egress
    // authorization and reported a synthetic "compiled" node path, so no *failing*
    // program was observably equivalent to the interpreter. These four tests pin the
    // widened envelope: budget halts, egress halts, and real failure-path node paths
    // (including runtime loop-iteration indices).

    #[test]
    fn budget_exhaustion_conforms() {
        // One model call with max_model_calls = 0: both backends halt with the same
        // BudgetExhausted terminal (no node path). Previously codegen ignored the
        // budget and "succeeded".
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![Node::new(
                    "classify",
                    NodeKind::Llm(LlmNode {
                        model: "mock".into(),
                        prompt_template: "{input}".into(),
                        temperature: 0.0,
                    }),
                )]),
            ),
        );
        let (interp, codegen) = run_both(
            &prog,
            Value::trusted(serde_json::json!({})),
            Some(crate::Budget {
                max_model_calls: 0,
                max_tool_calls: 10,
            }),
            || None,
        );
        assert_eq!(
            interp.status,
            RunStatus::BudgetExhausted {
                budget: "max_model_calls"
            },
            "{:?}",
            interp.status
        );
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn egress_denial_conforms() {
        // A pii-labeled value may not reach a provider unapproved for pii (IR-I9 /
        // PRD 7.5). Both backends must deny it at the SAME node path (root/0:agent)
        // with the SAME PolicyDenied reason — the authority half of the widened
        // envelope, which also exercises failure-path node-path parity.
        use entelechy_policy::{NativePolicy, PolicySnapshot, ProviderApproval};

        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![Node::new(
                    "agent",
                    NodeKind::Llm(LlmNode {
                        model: "mock-small".into(),
                        prompt_template: "{input}".into(),
                        temperature: 0.0,
                    }),
                )]),
            ),
        );
        let mut input = Value::trusted(serde_json::json!({}));
        input.meta.confidentiality.insert("pii".into());

        let mk_policy = || {
            let mut approval = ProviderApproval::new();
            approval.approve("internal", "mock-small"); // approved for internal, not pii
            Some(crate::PolicyConfig {
                engine: Box::new(NativePolicy::new()),
                authority: AuthorityEnvelope::empty(),
                snapshot: PolicySnapshot {
                    version: 1,
                    provider_approval: approval,
                },
                catalog: std::collections::HashMap::new(),
                principal: "runtime".into(),
            })
        };

        let (interp, codegen) = run_both(&prog, input, None, mk_policy);
        match &interp.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::PolicyDenied(_),
            } => assert_eq!(node_path, "root/0:agent", "node path {node_path}"),
            other => panic!("expected PolicyDenied at root/0:agent, got {other:?}"),
        }
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn failure_path_node_path_conforms() {
        // A Verify failure nested in a Seq: both backends report Failed at the SAME
        // real node path (root/1:check). Before this unit codegen reported the
        // synthetic "compiled" path, so no failing program was observably equivalent.
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
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "always_false".into(),
                        }),
                    ),
                ]),
            ),
        );
        let (interp, codegen) =
            run_both(&prog, Value::trusted(serde_json::json!({})), None, || None);
        match &interp.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::VerificationFailed(c),
            } => {
                assert_eq!(node_path, "root/1:check");
                assert_eq!(c, "always_false");
            }
            other => panic!("expected VerificationFailed at root/1:check, got {other:?}"),
        }
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn loop_body_failure_node_path_conforms() {
        // A failure inside a loop body must report the interpreter's runtime
        // iteration path (root/iter0:check) on both backends — proving the compiled
        // executor reconstructs loop/map indices into the node path at run time.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Loop {
                    body: Box::new(Node::new(
                        "check",
                        NodeKind::Verify(VerifyNode {
                            checker: "always_false".into(),
                        }),
                    )),
                    max_iters: 3,
                    until: Condition::Never,
                },
            ),
        );
        let (interp, codegen) =
            run_both(&prog, Value::trusted(serde_json::json!({})), None, || None);
        match &interp.status {
            RunStatus::Failed { node_path, .. } => {
                assert_eq!(node_path, "root/iter0:check", "node path {node_path}")
            }
            other => panic!("expected Failed at root/iter0:check, got {other:?}"),
        }
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    // ===== Conformance on the newly-lowered shapes: Gate / Tool / Delegate =====
    //
    // These shapes were interpreter-only until this unit. Each test drives the shape
    // through BOTH backends and asserts observable equivalence — including on the
    // halts they introduce (gate-blocked, tool budget, delegated-authority denial,
    // delegate widening). WS2 denials go through the shared `authorize_tool`, so the
    // backends report the identical reason string by construction.

    /// A program whose root `Seq` invokes a granted, deterministic consequential tool.
    fn tool_program() -> Program {
        let mut authority = AuthorityEnvelope::empty();
        authority.capabilities.insert("draft_reply".into());
        Program::new(
            authority,
            Node::new(
                "root",
                NodeKind::Seq(vec![Node::new(
                    "draft",
                    NodeKind::Tool(ToolNode {
                        capability: "draft_reply".into(),
                        args: serde_json::Value::Null,
                    }),
                )]),
            ),
        )
    }

    #[test]
    fn gate_open_declassifies_and_conforms() {
        // An opened gate (condition holds, no approval) declassifies the value to
        // Trusted for integrity (IR-I3). Feed a tainted value; both backends open it,
        // return Trusted, and agree.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Gate(GateNode {
                    policy: "allow".into(),
                    condition: Condition::Always,
                    requires_approval: false,
                }),
            ),
        );
        let (interp, codegen) = run_both(
            &prog,
            Value::tainted(serde_json::json!({ "x": 1 })),
            None,
            || None,
        );
        assert_eq!(interp.status, RunStatus::Succeeded, "{:?}", interp.status);
        assert_eq!(
            interp.output.as_ref().unwrap().meta.taint,
            entelechy_ir::Taint::Trusted,
            "an opened gate declassifies to Trusted"
        );
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn gate_blocked_conforms() {
        // A gate requiring an approval the Phase-0 runtime cannot discharge inline
        // blocks on both backends, at the same node path, with the same policy name.
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Seq(vec![Node::new(
                    "g",
                    NodeKind::Gate(GateNode {
                        policy: "needs-approval".into(),
                        condition: Condition::Always,
                        requires_approval: true,
                    }),
                )]),
            ),
        );
        let (interp, codegen) =
            run_both(&prog, Value::trusted(serde_json::json!({})), None, || None);
        match &interp.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::GateBlocked(p),
            } => {
                assert_eq!(node_path, "root/0:g");
                assert_eq!(p, "needs-approval");
            }
            other => panic!("expected GateBlocked at root/0:g, got {other:?}"),
        }
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn tool_effect_conforms() {
        // A granted consequential Tool dispatched through a deterministic gateway:
        // both backends authorize it, charge the tool budget, dispatch and return the
        // same egress output (the response, tainted + carrying the input's labels).
        let prog = tool_program();
        assert_conformant(&prog);
        let out = run_interp(&prog).output.unwrap();
        assert_eq!(
            out.data.get("reply").and_then(|r| r.as_str()),
            Some("hi"),
            "the tool response flows through"
        );
        assert_eq!(
            out.meta.taint,
            entelechy_ir::Taint::Tainted,
            "a tool output is external content (tainted)"
        );
    }

    #[test]
    fn tool_budget_exhaustion_conforms() {
        // max_tool_calls = 0: both backends halt with the same BudgetExhausted
        // terminal when the (authorized) tool is charged against the budget.
        let (interp, codegen) = run_both(
            &tool_program(),
            Value::trusted(serde_json::json!({})),
            Some(crate::Budget {
                max_model_calls: 10,
                max_tool_calls: 0,
            }),
            || None,
        );
        assert_eq!(
            interp.status,
            RunStatus::BudgetExhausted {
                budget: "max_tool_calls"
            },
            "{:?}",
            interp.status
        );
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn delegate_body_runs_and_conforms() {
        // A Delegate that narrows authority but whose body is a benign Code node runs
        // to success identically on both backends (the lowering preserves the happy
        // path; narrowing is observationally inert without a gated effect inside).
        let mut top = AuthorityEnvelope::empty();
        top.capabilities.insert("draft_reply".into());
        let prog = Program::new(
            top,
            Node::new(
                "root",
                NodeKind::Delegate {
                    authority: AuthorityEnvelope::empty(),
                    body: Box::new(Node::new(
                        "tag",
                        NodeKind::Code(CodeNode {
                            function: "tag_ok".into(),
                        }),
                    )),
                },
            ),
        );
        assert_conformant(&prog);
    }

    #[test]
    fn delegate_narrows_authority_denies_tool_conforms() {
        // WS2 (A2/IR-I6): a Tool inside a Delegate that narrowed the capability away is
        // denied at runtime on BOTH backends, at the same node path, with the identical
        // reason (both call the shared `authorize_tool`). The top-level program grants
        // the capability, so the same tool WITHOUT the delegate succeeds — see
        // `tool_effect_conforms` — the delegate is what denies it.
        let mut top = AuthorityEnvelope::empty();
        top.capabilities.insert("draft_reply".into());
        let prog = Program::new(
            top,
            Node::new(
                "root",
                NodeKind::Delegate {
                    authority: AuthorityEnvelope::empty(), // narrows draft_reply away
                    body: Box::new(Node::new(
                        "draft",
                        NodeKind::Tool(ToolNode {
                            capability: "draft_reply".into(),
                            args: serde_json::Value::Null,
                        }),
                    )),
                },
            ),
        );
        let (interp, codegen) =
            run_both(&prog, Value::trusted(serde_json::json!({})), None, || None);
        match &interp.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::AuthorityDenied(_),
            } => assert_eq!(node_path, "root/delegate:draft", "node path {node_path}"),
            other => panic!("expected AuthorityDenied at root/delegate:draft, got {other:?}"),
        }
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }

    #[test]
    fn delegate_widening_is_refused_conforms() {
        // A Delegate whose authority WIDENS the active envelope halts at the delegate
        // on both backends — defense-in-depth beyond the static IR-I6 check.
        let mut wide = AuthorityEnvelope::empty();
        wide.capabilities.insert("draft_reply".into()); // not a subset of the empty top
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "root",
                NodeKind::Delegate {
                    authority: wide,
                    body: Box::new(Node::new(
                        "tag",
                        NodeKind::Code(CodeNode {
                            function: "tag_ok".into(),
                        }),
                    )),
                },
            ),
        );
        let (interp, codegen) =
            run_both(&prog, Value::trusted(serde_json::json!({})), None, || None);
        match &interp.status {
            RunStatus::Failed {
                node_path,
                reason: FailureReason::AuthorityDenied(_),
            } => assert_eq!(node_path, "root", "node path {node_path}"),
            other => panic!("expected AuthorityDenied at root, got {other:?}"),
        }
        assert!(
            observable_equivalent(&interp, &codegen),
            "interp={interp:?} codegen={codegen:?}"
        );
    }
}
