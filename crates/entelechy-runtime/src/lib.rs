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

pub mod journal;

use std::collections::HashMap;

use entelechy_gateway::{ModelGateway, ModelRequest, ToolCall, ToolGateway};
use entelechy_ir::{Condition, MemOp, Node, NodeKind, Program, Value};

pub use journal::{
    CommitStatus, Journal, JournalEvent, JournalReader, ReplayMismatch,
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
enum Halt {
    Terminal(RunStatus),
}

/// The interpreter engine (PRD 8.1). Holds gateways and registries.
pub struct Engine<'a> {
    model: &'a dyn ModelGateway,
    tools: &'a mut dyn ToolGateway,
    checkers: HashMap<String, CheckerFn>,
    code: HashMap<String, CodeFn>,
    memory: HashMap<String, Value>,
    budget: Budget,
}

impl<'a> Engine<'a> {
    /// Build an engine over the given gateways.
    pub fn new(model: &'a dyn ModelGateway, tools: &'a mut dyn ToolGateway) -> Self {
        Self {
            model,
            tools,
            checkers: HashMap::new(),
            code: HashMap::new(),
            memory: HashMap::new(),
            budget: Budget::default(),
        }
    }

    /// Set execution budgets.
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
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
    pub fn replay(
        &mut self,
        program: &Program,
        input: Value,
        journal: &Journal,
    ) -> Result<Option<Value>, ReplayMismatch> {
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
}

#[derive(Default)]
struct Counters {
    model_calls: u32,
    tool_calls: u32,
}

enum Mode<'m, 'j> {
    Execute {
        journal: &'m mut Journal,
        counters: &'m mut Counters,
    },
    Replay {
        reader: &'m mut JournalReader<'j>,
        divergence: &'m mut Option<ReplayMismatch>,
    },
}

/// Recompute the final output on the successful path. In this functional-pipeline
/// slice the output is simply the value threaded to completion, so we re-run in
/// replay mode against the just-written journal to obtain it deterministically.
fn outcome_output(
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

fn initial_input() -> Value {
    Value::trusted(serde_json::json!({}))
}

fn exec_node(
    engine: &mut Engine,
    node: &Node,
    path: &str,
    value: Value,
    mode: &mut Mode,
) -> Result<Value, Halt> {
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
            let (branch, tag) = if take_then { (then, "then") } else { (els, "else") };
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
                    reason: FailureReason::CodeError(format!("unknown function '{}'", code.function)),
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
                .get(&mem.key)
                .cloned()
                .unwrap_or_else(|| Value::trusted(serde_json::Value::Null))),
            MemOp::Write => {
                engine.memory.insert(mem.key.clone(), value.clone());
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
                            Ok(Value::tainted(serde_json::json!({ "text": text })))
                        }
                        Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                            node_path: path.to_string(),
                            reason: FailureReason::ModelError(e.to_string()),
                        })),
                    }
                }
                Mode::Replay { reader, divergence } => match reader.next_for(path) {
                    Ok(JournalEvent::ModelCall { response, .. }) => {
                        Ok(Value::tainted(serde_json::json!({ "text": response.text })))
                    }
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
                            Ok(Value::tainted(response))
                        }
                        Err(e) => Err(Halt::Terminal(RunStatus::Failed {
                            node_path: path.to_string(),
                            reason: FailureReason::ToolError(e.to_string()),
                        })),
                    }
                }
                Mode::Replay { reader, divergence } => match reader.next_for(path) {
                    Ok(JournalEvent::ToolEffect { response, .. }) => {
                        Ok(Value::tainted(response.clone()))
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
            }
        }
    }
}

fn journal_run_id(mode: &Mode) -> String {
    match mode {
        Mode::Execute { journal, .. } => journal.run_id.clone(),
        Mode::Replay { .. } => "replay".to_string(),
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
        Condition::Equals { field, value: expected } => {
            get_path(&value.data, field) == Some(expected)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_gateway::{MockModel, NativeToolGateway};
    use entelechy_ir::{
        AuthorityEnvelope, CodeNode, LlmNode, Node, NodeKind, Program, VerifyNode,
    };

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
        let result = e.execute(&demo_program(), Value::trusted(serde_json::json!({})), "run-1");
        assert_eq!(result.status, RunStatus::Succeeded, "{:?}", result.status);
        // One model call journaled.
        assert_eq!(result.journal.entries.len(), 1);
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
            .replay(&prog, Value::trusted(serde_json::json!({})), &result.journal)
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
            Node::new("v", NodeKind::Verify(VerifyNode { checker: "always_false".into() })),
        );
        let r = e.execute(&prog, Value::trusted(serde_json::json!({})), "run-x");
        assert!(matches!(
            r.status,
            RunStatus::Failed { reason: FailureReason::VerificationFailed(_), .. }
        ));
        assert!(r.output.is_none());
    }

    #[test]
    fn model_budget_exhaustion_is_typed_terminal() {
        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();
        let mut e = Engine::new(&model, &mut tools)
            .with_budget(Budget { max_model_calls: 0, max_tool_calls: 10 });
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
        assert_eq!(r.status, RunStatus::BudgetExhausted { budget: "max_model_calls" });
    }
}
