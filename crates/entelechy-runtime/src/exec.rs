//! Interpreter execution: the tree-walking `exec_node`, its execution `Mode`
//! (execute / replay / counterfactual) and the value-flow helpers (PRD 8, 10.2).
//!
//! Split out of `lib.rs` for readability. `Engine`'s public `execute`, `replay`
//! and `counterfactual` methods drive these; child-module access to `Engine`'s
//! private fields and `authorize_*` methods is intentional.

use entelechy_gateway::{ModelRequest, ToolCall};
use entelechy_ir::{Condition, MemOp, Node, NodeKind, Program, Value, ValueMeta};

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

#[derive(Default)]
pub(crate) struct Counters {
    model_calls: u32,
    tool_calls: u32,
}

pub(crate) enum Mode<'m, 'j> {
    Execute {
        journal: &'m mut Journal,
        counters: &'m mut Counters,
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
    },
}

/// Recompute the final output on the successful path. In this functional-pipeline
/// slice the output is simply the value threaded to completion, so we re-run in
/// replay mode against the just-written journal to obtain it deterministically.
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
    match exec_node(engine, &program.root, "root", initial_input(), &mut mode) {
        Ok(v) if divergence.is_none() => Some(v),
        _ => None,
    }
}

pub(crate) fn initial_input() -> Value {
    Value::trusted(serde_json::json!({}))
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
            // as an array (recorded order == index order, RK-7).
            let mut out = Vec::new();
            for (i, child) in children.iter().enumerate() {
                let child_path = format!("{path}/{i}:{}", child.id);
                let r = exec_node(engine, child, &child_path, value.clone(), mode)?;
                out.push(r.data);
            }
            Ok(Value::tainted(serde_json::Value::Array(out)))
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
            let mut out = Vec::new();
            for (i, item) in items.into_iter().enumerate() {
                let child_path = format!("{path}/map{i}:{}", body.id);
                let r = exec_node(engine, body, &child_path, Value::trusted(item), mode)?;
                out.push(r.data);
            }
            Ok(Value::trusted(serde_json::Value::Array(out)))
        }
        NodeKind::Delegate { body, .. } => {
            // Authority narrowing is verified statically (IR-I6). Execute the body.
            let child_path = format!("{path}/delegate:{}", body.id);
            exec_node(engine, body, &child_path, value, mode)
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
                Mode::Execute { journal, counters } => {
                    // Model calls are egress (PRD 7.5): enforce confidentiality
                    // provider routing before dispatch (IR-I9).
                    if let Err(reason) =
                        engine.authorize_model(&llm.model, &value.meta.confidentiality)
                    {
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
                    ..
                } => {
                    if **diverged {
                        // Live re-execution downstream of the divergence (RK-8).
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
                Mode::Execute { journal, counters } => {
                    // Authorize the effect at its boundary before dispatch (PRD 8.3).
                    if let Err(reason) = engine.authorize_tool(&tool.capability, value.meta.taint) {
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
                    let call = ToolCall {
                        capability: tool.capability.clone(),
                        args: merge_args(&tool.args, &value.data),
                        operation_key: operation_key.clone(),
                    };
                    match engine.tools.call(&call) {
                        Ok(response) => {
                            journal.append(
                                path,
                                JournalEvent::ToolEffect {
                                    capability: tool.capability.clone(),
                                    operation_key,
                                    request_hash,
                                    response: response.clone(),
                                    commit: CommitStatus::Committed,
                                },
                            );
                            Ok(egress_output(response, &value.meta))
                        }
                        Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                            node_path: path.to_string(),
                            reason: FailureReason::ToolError(e.to_string()),
                        })),
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
                    ..
                } => {
                    if **diverged {
                        // Live re-execution downstream of the divergence (RK-8).
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
fn merge_args(args: &serde_json::Value, input: &serde_json::Value) -> serde_json::Value {
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
fn eval_condition(cond: &Condition, value: &Value) -> bool {
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

/// Resolve a dot-path within a JSON value.
fn get_path<'v>(value: &'v serde_json::Value, path: &str) -> Option<&'v serde_json::Value> {
    let mut cur = value;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}
