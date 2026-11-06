//! Interpreter execution: the tree-walking `exec_node`, its execution `Mode`
//! (execute / replay / counterfactual) and the value-flow helpers (PRD 8, 10.2).
//!
//! Split out of `lib.rs` for readability. `Engine`'s public `execute`, `replay`
//! and `counterfactual` methods drive these; child-module access to `Engine`'s
//! private fields and `authorize_*` methods is intentional.

use entelechy_gateway::{EffectOutcome, ModelRequest, ToolCall};
use entelechy_ir::{
    Condition, Confidentiality, MemOp, Node, NodeKind, Program, Taint, Value, ValueMeta,
};

use crate::journal::{Journal, JournalEvent, JournalReader, ReplayMismatch};
use crate::{CommitStatus, Engine, FailureReason, Halt, RunStatus};

/// The result of a counterfactual replay (PRD 10.2, RK-8).
#[derive(Clone, Debug, PartialEq)]
pub struct CounterfactualResult {
    /// The node at which execution diverged from the recording, if reached.
    pub divergence_point: Option<String>,
    /// Number of live re-executions after the divergence point. A single rollout
    /// is weak evidence (PRD 10.2).
    pub rollouts: u32,
    /// Whether the intervention was reached and applied.
    pub diverged: bool,
    /// The counterfactual output value, if the run completed.
    pub output: Option<Value>,
}

/// A consequential tool effect intercepted and *substituted* by a shadow run
/// (PRD §14.2 L3 effect substitution). Records what the engine *would* have
/// dispatched; in shadow the engine never calls the tool gateway, so no such effect
/// committed. Surfaced out of band in [`crate::ShadowResult`] rather than in the
/// replay journal, so a shadow run leaves the journal wire format untouched.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SubstitutedEffect {
    /// The capability the intercepted tool node would have invoked.
    pub capability: String,
    /// The idempotency operation key the real dispatch would have used (PRD 7.6).
    pub operation_key: String,
    /// Hash of the canonical request body (PRD 7.6) — identifies the substituted call.
    pub request_hash: String,
    /// The node path of the intercepted tool node.
    pub path: String,
}

#[derive(Default)]
pub(crate) struct Counters {
    model_calls: u32,
    tool_calls: u32,
}

pub(crate) enum Mode<'m, 'j> {
    Execute {
        journal: &'m mut Journal,
        counters: &'m mut Counters,
        /// Shadow interception sink (PRD §14.2 L3). `Some` marks a shadow run: every
        /// consequential `Tool` effect is authorized and budgeted exactly as a real
        /// one, then *substituted* — recorded here and given a record-only response —
        /// instead of dispatched, so `ToolGateway::call` is never invoked and the run
        /// commits no consequential effect. `None` is an ordinary run that dispatches.
        shadow: Option<&'m mut Vec<SubstitutedEffect>>,
    },
    Replay {
        reader: &'m mut JournalReader<'j>,
        divergence: &'m mut Option<ReplayMismatch>,
    },
    /// Counterfactual replay (PRD 10.2, RK-8): consume recorded effects until the
    /// intervention node, substitute its output, then re-execute live downstream.
    Counterfactual {
        reader: &'m mut JournalReader<'j>,
        diverged: &'m mut bool,
        divergence_point: &'m mut Option<String>,
        rollouts: &'m mut u32,
        intervention_path: &'m str,
        intervention_value: &'m Value,
        /// Budget counters for the live (post-divergence) span, so a counterfactual
        /// re-execution is charged the same tool/model budget as a real run
        /// (PRD 10.2 / RK-8: counterfactual is not a privileged path).
        counters: &'m mut Counters,
    },
}

/// Pre-dispatch authorization + budget guard for a model (Llm) effect (PRD 7.5,
/// 8.3, IR-I9). Confidentiality provider routing runs first, then the model-call
/// budget is charged and checked. Shared by the `Execute` arm and the live
/// (post-divergence) `Counterfactual` arm so a counterfactual model call passes the
/// same gates as a real one (RK-8 — no privileged counterfactual path).
fn guard_model(
    engine: &mut Engine,
    model: &str,
    confidentiality: &Confidentiality,
    path: &str,
    counters: &mut Counters,
) -> Result<(), Halt> {
    if let Err(reason) = engine.authorize_model(model, confidentiality) {
        return Err(Halt::Terminal(RunStatus::Failed {
            node_path: path.to_string(),
            reason,
        }));
    }
    counters.model_calls += 1;
    if counters.model_calls > engine.budget.max_model_calls {
        return Err(Halt::Terminal(RunStatus::BudgetExhausted {
            budget: "max_model_calls",
        }));
    }
    Ok(())
}

/// Pre-dispatch authorization + budget guard for a consequential tool effect
/// (PRD 8.3). The effect is authorized at its boundary, then the tool-call budget
/// is charged and checked. Shared by the `Execute` arm and the live
/// (post-divergence) `Counterfactual` arm (RK-8 — no privileged counterfactual path).
fn guard_tool(
    engine: &mut Engine,
    capability: &str,
    taint: Taint,
    path: &str,
    counters: &mut Counters,
) -> Result<(), Halt> {
    if let Err(reason) = engine.authorize_tool(capability, taint) {
        return Err(Halt::Terminal(RunStatus::Failed {
            node_path: path.to_string(),
            reason,
        }));
    }
    counters.tool_calls += 1;
    if counters.tool_calls > engine.budget.max_tool_calls {
        return Err(Halt::Terminal(RunStatus::BudgetExhausted {
            budget: "max_tool_calls",
        }));
    }
    Ok(())
}

/// Recompute the final output on the successful path. In this functional-pipeline
/// slice the output is simply the value threaded to completion, so we re-run in
/// replay mode against the just-written journal to obtain it deterministically.
/// The replay is seeded from the journal's recorded initial input (RK-2), so a
/// node that reads the input reproduces the real threaded output rather than the
/// output of an empty `{}` run.
pub(crate) fn outcome_output(
    engine: &mut Engine,
    program: &Program,
    _run_id: &str,
    journal: &Journal,
) -> Option<Value> {
    // Re-run against our own journal to capture the threaded output value.
    let mut reader = JournalReader::new(journal);
    let mut divergence = None;
    let mut mode = Mode::Replay {
        reader: &mut reader,
        divergence: &mut divergence,
    };
    match exec_node(
        engine,
        &program.root,
        "root",
        journal.input.clone(),
        &mut mode,
    ) {
        Ok(v) if divergence.is_none() => Some(v),
        _ => None,
    }
}

/// Build a tainted egress output that inherits the input's confidentiality labels
/// (PRD 7.5: an output inherits the union of its input labels unless a Verify or
/// declassification policy lowers it).
pub(crate) fn egress_output(data: serde_json::Value, from: &ValueMeta) -> Value {
    let mut v = Value::tainted(data);
    v.meta.confidentiality = from.confidentiality.clone();
    v
}

pub(crate) fn exec_node(
    engine: &mut Engine,
    node: &Node,
    path: &str,
    value: Value,
    mode: &mut Mode,
) -> Result<Value, Halt> {
    // Counterfactual intervention (PRD 10.2): at the intervention node, substitute
    // its output and diverge; the subtree is not executed and everything after
    // runs live.
    if let Mode::Counterfactual {
        diverged,
        divergence_point,
        intervention_path,
        intervention_value,
        ..
    } = mode
    {
        if !**diverged && *intervention_path == path {
            **diverged = true;
            **divergence_point = Some(path.to_string());
            return Ok((**intervention_value).clone());
        }
    }

    match &node.kind {
        NodeKind::Seq(children) => {
            let mut cur = value;
            for (i, child) in children.iter().enumerate() {
                let child_path = format!("{path}/{i}:{}", child.id);
                cur = exec_node(engine, child, &child_path, cur, mode)?;
            }
            Ok(cur)
        }
        NodeKind::Par(children) => {
            // Deterministic sequential evaluation in this slice; results collected
            // as an array (recorded order == index order, RK-7). The array inherits
            // the join of the input's and every child's labels, so a Par never
            // launders confidentiality and never over-taints a fully-trusted fan-out
            // (IR-I9 / PRD 7.5 — same join Retrieval uses).
            let mut out = Vec::with_capacity(children.len());
            let mut meta = value.meta.clone();
            for (i, child) in children.iter().enumerate() {
                let child_path = format!("{path}/{i}:{}", child.id);
                let r = exec_node(engine, child, &child_path, value.clone(), mode)?;
                meta = meta.join(&r.meta);
                out.push(r.data);
            }
            Ok(Value {
                data: serde_json::Value::Array(out),
                meta,
            })
        }
        NodeKind::Branch { cond, then, els } => {
            let take_then = eval_condition(cond, &value);
            let (branch, tag) = if take_then {
                (then, "then")
            } else {
                (els, "else")
            };
            let child_path = format!("{path}/{tag}:{}", branch.id);
            exec_node(engine, branch, &child_path, value, mode)
        }
        NodeKind::Loop {
            body,
            max_iters,
            until,
        } => {
            let mut cur = value;
            for i in 0..*max_iters {
                if eval_condition(until, &cur) {
                    break;
                }
                let child_path = format!("{path}/iter{i}:{}", body.id);
                cur = exec_node(engine, body, &child_path, cur, mode)?;
            }
            Ok(cur)
        }
        NodeKind::Map { over, body } => {
            let items = get_path(&value.data, over)
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            // Each element is no more trusted than the collection it came from, so it
            // inherits the collection's labels; the result joins the collection and
            // every body output. A Map therefore never launders taint/confidentiality
            // (IR-I9 / PRD 7.5), matching the static model, which also threads taint
            // through the Map body (entelechy_ir invariants).
            let mut out = Vec::with_capacity(items.len());
            let mut meta = value.meta.clone();
            for (i, item) in items.into_iter().enumerate() {
                let child_path = format!("{path}/map{i}:{}", body.id);
                let item_value = Value {
                    data: item,
                    meta: value.meta.clone(),
                };
                let r = exec_node(engine, body, &child_path, item_value, mode)?;
                meta = meta.join(&r.meta);
                out.push(r.data);
            }
            Ok(Value {
                data: serde_json::Value::Array(out),
                meta,
            })
        }
        NodeKind::Delegate { authority, body } => {
            // Runtime authority narrowing (A2 / IR-I6, PRD 5.4): a `Delegate` may
            // only narrow the active envelope, so a widening delegate halts at
            // runtime — defense-in-depth beyond the static IR-I6 check. The narrowed
            // envelope is pushed for the body and popped afterwards (even on a halt,
            // so the stack stays balanced), so every effect inside the body is
            // authorized against it by `authorize_tool` (WS2).
            let widens = engine
                .authority_stack
                .last()
                .map(|active| !authority.is_subset_of(active))
                .unwrap_or(false);
            if widens {
                return Err(Halt::Terminal(RunStatus::Failed {
                    node_path: path.to_string(),
                    reason: FailureReason::AuthorityDenied(
                        "delegated authority widens the active envelope (A2/IR-I6)".into(),
                    ),
                }));
            }
            engine.authority_stack.push(authority.clone());
            let child_path = format!("{path}/delegate:{}", body.id);
            let result = exec_node(engine, body, &child_path, value, mode);
            engine.authority_stack.pop();
            result
        }
        NodeKind::Code(code) => {
            let f = engine.code.get(&code.function).ok_or_else(|| {
                Halt::Terminal(RunStatus::Failed {
                    node_path: path.to_string(),
                    reason: FailureReason::CodeError(format!(
                        "unknown function '{}'",
                        code.function
                    )),
                })
            })?;
            match f(&value) {
                Ok(data) => Ok(Value {
                    data,
                    meta: value.meta,
                }),
                Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                    node_path: path.to_string(),
                    reason: FailureReason::CodeError(e),
                })),
            }
        }
        NodeKind::Verify(v) => {
            let checker = engine.checkers.get(&v.checker).ok_or_else(|| {
                Halt::Terminal(RunStatus::Failed {
                    node_path: path.to_string(),
                    reason: FailureReason::VerificationFailed(format!(
                        "unknown checker '{}'",
                        v.checker
                    )),
                })
            })?;
            if checker(&value) {
                let mut out = value;
                out.meta.verification = entelechy_ir::VerificationState::Verified;
                Ok(out)
            } else {
                Err(Halt::Terminal(RunStatus::Failed {
                    node_path: path.to_string(),
                    reason: FailureReason::VerificationFailed(v.checker.clone()),
                }))
            }
        }
        NodeKind::Gate(gate) => {
            let allowed = eval_condition(&gate.condition, &value) && !gate.requires_approval;
            match mode {
                Mode::Execute { journal, .. } => {
                    journal.append(
                        path,
                        JournalEvent::PolicyDecision {
                            policy: gate.policy.clone(),
                            allowed,
                        },
                    );
                }
                Mode::Replay { reader, divergence } => {
                    if let Err(d) = reader.next_for(path) {
                        divergence.get_or_insert(d);
                    }
                }
                Mode::Counterfactual {
                    reader, diverged, ..
                } => {
                    // Before divergence, follow the recorded decision; after, the
                    // gate is evaluated live (no journal consumed).
                    if !**diverged {
                        let _ = reader.next_for(path);
                    }
                }
            }
            if allowed {
                // Downstream of an opened gate, the value is declassified for
                // integrity (IR-I3): mark it trusted.
                let mut out = value;
                out.meta.taint = entelechy_ir::Taint::Trusted;
                Ok(out)
            } else {
                Err(Halt::Terminal(RunStatus::Failed {
                    node_path: path.to_string(),
                    reason: FailureReason::GateBlocked(gate.policy.clone()),
                }))
            }
        }
        NodeKind::Mem(mem) => match mem.op {
            MemOp::Read => Ok(engine
                .memory
                .read(mem.tier, &mem.key)
                .cloned()
                .unwrap_or_else(|| Value::trusted(serde_json::Value::Null))),
            MemOp::Write => {
                engine
                    .memory
                    .write(mem.tier, mem.key.clone(), value.clone());
                Ok(value)
            }
        },
        NodeKind::Retrieval(r) => {
            // Build the query the same way an Llm builds its prompt, then read the
            // top_k most relevant entries of the tier (PRD 12, 11.4 level 2).
            let query = r.query_template.replace("{input}", &value.data.to_string());
            let hits = engine.memory.retrieve(r.tier, &query, r.top_k);
            // The result is external content: tainted (PRD 5.8), inheriting the
            // input's labels joined with every retrieved entry's labels so
            // confidentiality propagates downstream (PRD 7.5 / IR-I9). Retrieval is
            // a deterministic in-process read, so — like a Mem node — it records no
            // journal event and reproduces exactly on replay/counterfactual (RK-2).
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
        NodeKind::Human(_) => Err(Halt::Terminal(RunStatus::Failed {
            node_path: path.to_string(),
            reason: FailureReason::Unsupported(
                "Human nodes require an interactive responder (Phase 2+)".into(),
            ),
        })),
        NodeKind::Llm(llm) => {
            let prompt = llm
                .prompt_template
                .replace("{input}", &value.data.to_string());
            let request = ModelRequest {
                model: llm.model.clone(),
                prompt,
                temperature: llm.temperature,
            };
            match mode {
                Mode::Execute {
                    journal, counters, ..
                } => {
                    // Egress routing (IR-I9) + budget before dispatch (PRD 7.5, 8.3).
                    // A model (inference) call is not a consequential world-effect, so
                    // it runs live in shadow too — the shadow outcome must be
                    // comparable to the baseline's (PRD §14.2).
                    guard_model(
                        engine,
                        &llm.model,
                        &value.meta.confidentiality,
                        path,
                        counters,
                    )?;
                    match engine.model.infer(&request) {
                        Ok(response) => {
                            let text = response.text.clone();
                            journal.append(path, JournalEvent::ModelCall { request, response });
                            Ok(egress_output(
                                serde_json::json!({ "text": text }),
                                &value.meta,
                            ))
                        }
                        Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                            node_path: path.to_string(),
                            reason: FailureReason::ModelError(e.to_string()),
                        })),
                    }
                }
                Mode::Replay { reader, divergence } => match reader.next_for(path) {
                    Ok(JournalEvent::ModelCall { response, .. }) => Ok(egress_output(
                        serde_json::json!({ "text": response.text }),
                        &value.meta,
                    )),
                    Ok(_) => {
                        divergence.get_or_insert(ReplayMismatch::Divergence {
                            expected: "model-call".into(),
                            actual: path.to_string(),
                            seq: 0,
                        });
                        Ok(Value::tainted(serde_json::json!({ "text": "" })))
                    }
                    Err(d) => {
                        divergence.get_or_insert(d);
                        Ok(Value::tainted(serde_json::json!({ "text": "" })))
                    }
                },
                Mode::Counterfactual {
                    reader,
                    diverged,
                    rollouts,
                    counters,
                    ..
                } => {
                    if **diverged {
                        // Live re-execution downstream of the divergence passes the
                        // same egress-routing + budget gate as a real model call
                        // (RK-8 / PRD 10.2): counterfactual is not privileged.
                        guard_model(
                            engine,
                            &llm.model,
                            &value.meta.confidentiality,
                            path,
                            counters,
                        )?;
                        **rollouts += 1;
                        match engine.model.infer(&request) {
                            Ok(response) => Ok(egress_output(
                                serde_json::json!({ "text": response.text }),
                                &value.meta,
                            )),
                            Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                                node_path: path.to_string(),
                                reason: FailureReason::ModelError(e.to_string()),
                            })),
                        }
                    } else {
                        match reader.next_for(path) {
                            Ok(JournalEvent::ModelCall { response, .. }) => Ok(egress_output(
                                serde_json::json!({ "text": response.text }),
                                &value.meta,
                            )),
                            _ => Ok(egress_output(
                                serde_json::json!({ "text": "" }),
                                &value.meta,
                            )),
                        }
                    }
                }
            }
        }
        NodeKind::Tool(tool) => {
            let operation_key = format!("{}/{}/attempt0", journal_run_id(mode), path);
            let request_body = serde_json::json!({
                "capability": tool.capability,
                "args": tool.args,
            });
            let request_hash = entelechy_artifacts::ArtifactId::of(&request_body)
                .map(|id| id.to_string())
                .unwrap_or_default();
            match mode {
                Mode::Execute {
                    journal,
                    counters,
                    shadow,
                } => {
                    // Authorize the effect at its boundary + charge budget before
                    // dispatch (PRD 8.3). A shadow run passes the same WP1 authority
                    // (WS2) and budget (WS3) gates as a real one.
                    guard_tool(engine, &tool.capability, value.meta.taint, path, counters)?;
                    if let Some(sink) = shadow.as_deref_mut() {
                        // WS-shadow (PRD §14.2 L3 effect substitution): intercept the
                        // effect. `engine.tools.call` is NOT invoked, so no consequential
                        // effect commits — a *runtime* guarantee, independent of the
                        // capability's declared class or `dry_run_supported` metadata.
                        // Nothing is journaled as a ToolEffect (no effect occurred), so a
                        // shadow run's journal carries zero committed effects (the
                        // structural proof) and the replay journal stays byte-identical
                        // to v2. The intercepted call is recorded out of band for
                        // evidence.
                        sink.push(SubstitutedEffect {
                            capability: tool.capability.clone(),
                            operation_key,
                            request_hash,
                            path: path.to_string(),
                        });
                        // A deterministic record-only value flows downstream (the
                        // dry-run sink), carrying the input's labels so IFC still
                        // propagates (IR-I9).
                        return Ok(egress_output(serde_json::Value::Null, &value.meta));
                    }
                    let call = ToolCall {
                        capability: tool.capability.clone(),
                        args: merge_args(&tool.args, &value.data),
                        operation_key: operation_key.clone(),
                    };
                    // WS5 — write-ahead intent (PRD 7.6, 8.5): record a durable
                    // intent (commit = Unknown) *before* dispatch, so a crash or a
                    // lost acknowledgement leaves a dangling Unknown effect in the
                    // journal that forces the run into `ReconciliationRequired`
                    // (PRD 8.6). The entry transitions Unknown -> Committed in place
                    // on acknowledgement, so a completed run still holds exactly one
                    // effect per tool node (replay-stable, byte-identical to before).
                    let intent = journal.entries.len();
                    journal.append(
                        path,
                        JournalEvent::ToolEffect {
                            capability: tool.capability.clone(),
                            operation_key,
                            request_hash,
                            response: serde_json::Value::Null,
                            commit: CommitStatus::Unknown,
                        },
                    );
                    match engine.tools.call(&call) {
                        Ok(response) => {
                            // Acknowledged: resolve the intent in place to Committed
                            // with the real response.
                            if let Some(JournalEvent::ToolEffect {
                                response: slot,
                                commit,
                                ..
                            }) = journal.entries.get_mut(intent).map(|e| &mut e.event)
                            {
                                *slot = response.clone();
                                *commit = CommitStatus::Committed;
                            }
                            Ok(egress_output(response, &value.meta))
                        }
                        // A failed dispatch splits by effect outcome (PRD 8.5/8.6):
                        // a definitive rejection committed nothing, so the run fails
                        // cleanly; a lost or indeterminate acknowledgement may have
                        // taken hold, so the run must reconcile. The runtime never
                        // declares success for, nor silently discards, an effect whose
                        // outcome it cannot confirm.
                        Err(e) => match e.effect_outcome() {
                            // Nothing committed: drop the speculative write-ahead
                            // intent (truncating removes exactly the one entry appended
                            // just above) — there is no dangling effect to reconcile —
                            // and fail cleanly with the rejection reason.
                            EffectOutcome::NoEffect => {
                                journal.entries.truncate(intent);
                                Err(Halt::Terminal(RunStatus::Failed {
                                    node_path: path.to_string(),
                                    reason: FailureReason::ToolError(e.to_string()),
                                }))
                            }
                            // Possible effect: leave the Unknown intent dangling so the
                            // run ends in `ReconciliationRequired` (unchanged behavior).
                            EffectOutcome::Indeterminate => {
                                Err(Halt::Terminal(RunStatus::ReconciliationRequired))
                            }
                        },
                    }
                }
                Mode::Replay { reader, divergence } => match reader.next_for(path) {
                    Ok(JournalEvent::ToolEffect { response, .. }) => {
                        Ok(egress_output(response.clone(), &value.meta))
                    }
                    Ok(_) => {
                        divergence.get_or_insert(ReplayMismatch::Divergence {
                            expected: "tool-effect".into(),
                            actual: path.to_string(),
                            seq: 0,
                        });
                        Ok(Value::tainted(serde_json::Value::Null))
                    }
                    Err(d) => {
                        divergence.get_or_insert(d);
                        Ok(Value::tainted(serde_json::Value::Null))
                    }
                },
                Mode::Counterfactual {
                    reader,
                    diverged,
                    rollouts,
                    counters,
                    ..
                } => {
                    if **diverged {
                        // Live re-execution downstream of the divergence passes the
                        // same authorization + budget gate as a real effect (RK-8 /
                        // PRD 10.2): counterfactual is not privileged.
                        guard_tool(engine, &tool.capability, value.meta.taint, path, counters)?;
                        **rollouts += 1;
                        let call = ToolCall {
                            capability: tool.capability.clone(),
                            args: merge_args(&tool.args, &value.data),
                            operation_key,
                        };
                        match engine.tools.call(&call) {
                            Ok(response) => Ok(egress_output(response, &value.meta)),
                            Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                                node_path: path.to_string(),
                                reason: FailureReason::ToolError(e.to_string()),
                            })),
                        }
                    } else {
                        match reader.next_for(path) {
                            Ok(JournalEvent::ToolEffect { response, .. }) => {
                                Ok(egress_output(response.clone(), &value.meta))
                            }
                            _ => Ok(Value::tainted(serde_json::Value::Null)),
                        }
                    }
                }
            }
        }
    }
}

fn journal_run_id(mode: &Mode) -> String {
    match mode {
        Mode::Execute { journal, .. } => journal.run_id.clone(),
        Mode::Replay { .. } => "replay".to_string(),
        Mode::Counterfactual { .. } => "counterfactual".to_string(),
    }
}

/// Merge static tool args with the current value's payload (static args win).
/// `pub(crate)` so the compiled backend (`codegen`) builds the identical `ToolCall`
/// the interpreter dispatches, for interpreter-vs-codegen parity (Q10).
pub(crate) fn merge_args(args: &serde_json::Value, input: &serde_json::Value) -> serde_json::Value {
    match (args, input) {
        (serde_json::Value::Object(a), serde_json::Value::Object(i)) => {
            let mut merged = i.clone();
            for (k, v) in a {
                merged.insert(k.clone(), v.clone());
            }
            serde_json::Value::Object(merged)
        }
        (serde_json::Value::Null, i) => i.clone(),
        (a, _) => a.clone(),
    }
}

/// Evaluate a typed condition against a value (PRD 7.1 Branch / 7.3 Gate).
/// `pub(crate)` so the compiled backend (`codegen`) evaluates branch/loop
/// conditions through the identical logic, for interpreter-vs-codegen parity (Q10).
pub(crate) fn eval_condition(cond: &Condition, value: &Value) -> bool {
    match cond {
        Condition::Always => true,
        Condition::Never => false,
        Condition::Truthy { field } => match get_path(&value.data, field) {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::Null) | None => false,
            Some(serde_json::Value::Number(n)) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
            Some(serde_json::Value::String(s)) => !s.is_empty(),
            Some(serde_json::Value::Array(a)) => !a.is_empty(),
            Some(serde_json::Value::Object(o)) => !o.is_empty(),
        },
        Condition::Equals {
            field,
            value: expected,
        } => get_path(&value.data, field) == Some(expected),
        Condition::IsTainted => value.meta.taint == entelechy_ir::Taint::Tainted,
        Condition::Not(inner) => !eval_condition(inner, value),
    }
}

/// Resolve a dot-path within a JSON value. `pub(crate)` so the compiled backend
/// (`codegen`) resolves `Map` collections identically to the interpreter (Q10).
pub(crate) fn get_path<'v>(
    value: &'v serde_json::Value,
    path: &str,
) -> Option<&'v serde_json::Value> {
    let mut cur = value;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}
