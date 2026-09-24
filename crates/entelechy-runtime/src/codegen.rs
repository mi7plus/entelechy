//! Compiled execution backend and the codegen conformance/promotion gate
//! (PRD 21 Phase 4, Q10, risk "runtime/codegen divergence").
//!
//! Codegen is optional: it becomes a production runtime only when it passes the
//! shared conformance suite, replays >=1,000 journals identically, shows no
//! holdout regression, and cuts p95 latency or cost by >=30% on >=3 objectives
//! (Q10); otherwise the interpreter stays the production runtime.
//!
//! This module provides the substrate for that decision without a full
//! source-emitting compiler: [`CompiledPlan::compile`] lowers a linear IR program
//! to a flat op list (a real compilation step distinct from the tree-walking
//! interpreter), [`Engine::run_compiled`] executes it, [`observable_equivalent`]
//! checks interpreter-vs-codegen conformance, and [`CodegenEvidence`] encodes the
//! Q10 promotion criteria. A richer backend (e.g. emitting Rust) implements the
//! same lowering and must satisfy the same conformance gate.

use serde::{Deserialize, Serialize};

use entelechy_ir::{AuthorityEnvelope, MemOp, MemoryTier, NodeKind, Program, Value};

use crate::journal::Journal;
use crate::{egress_output, Engine, FailureReason, RunResult, RunStatus};

/// A lowered, flat instruction (the "compiled" form of a supported IR node).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompiledStep {
    /// Model inference.
    Llm {
        /// Model identity.
        model: String,
        /// Prompt template.
        prompt_template: String,
        /// Sampling temperature.
        temperature: f64,
    },
    /// Deterministic code function.
    Code {
        /// Registered function name.
        function: String,
    },
    /// Verification checker.
    Verify {
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
}

/// A compiled program: a flat op list plus the authority envelope (PRD Q10).
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
    /// Lower a linear IR program (a root `Seq` of supported leaf nodes, or a
    /// single supported leaf) into a compiled plan. Unsupported shapes return
    /// `Unsupported`, so the interpreter stays the runtime for them (Q10).
    pub fn compile(program: &Program) -> Result<CompiledPlan, CompileError> {
        let mut steps = Vec::new();
        match &program.root.kind {
            NodeKind::Seq(children) => {
                for child in children {
                    steps.push(lower(&child.kind)?);
                }
            }
            other => steps.push(lower(other)?),
        }
        Ok(CompiledPlan {
            authority: program.authority.clone(),
            steps,
        })
    }
}

fn lower(kind: &NodeKind) -> Result<CompiledStep, CompileError> {
    Ok(match kind {
        NodeKind::Llm(l) => CompiledStep::Llm {
            model: l.model.clone(),
            prompt_template: l.prompt_template.clone(),
            temperature: l.temperature,
        },
        NodeKind::Code(c) => CompiledStep::Code {
            function: c.function.clone(),
        },
        NodeKind::Verify(v) => CompiledStep::Verify {
            checker: v.checker.clone(),
        },
        NodeKind::Mem(m) => match m.op {
            MemOp::Read => CompiledStep::MemRead {
                tier: m.tier,
                key: m.key.clone(),
            },
            MemOp::Write => CompiledStep::MemWrite {
                tier: m.tier,
                key: m.key.clone(),
            },
        },
        NodeKind::Seq(_) => return Err(CompileError::Unsupported("seq")),
        NodeKind::Par(_) => return Err(CompileError::Unsupported("par")),
        NodeKind::Map { .. } => return Err(CompileError::Unsupported("map")),
        NodeKind::Branch { .. } => return Err(CompileError::Unsupported("branch")),
        NodeKind::Loop { .. } => return Err(CompileError::Unsupported("loop")),
        NodeKind::Delegate { .. } => return Err(CompileError::Unsupported("delegate")),
        NodeKind::Tool(_) => return Err(CompileError::Unsupported("tool")),
        NodeKind::Gate(_) => return Err(CompileError::Unsupported("gate")),
        NodeKind::Human(_) => return Err(CompileError::Unsupported("human")),
    })
}

impl Engine<'_> {
    /// Execute a [`CompiledPlan`] (the codegen backend). Produces the same
    /// observable state as the interpreter for the supported subset — verified by
    /// [`observable_equivalent`] against `execute` (Q10 conformance).
    pub fn run_compiled(&mut self, plan: &CompiledPlan, input: Value, run_id: &str) -> RunResult {
        let mut value = input;
        for step in &plan.steps {
            match step {
                CompiledStep::Llm {
                    model,
                    prompt_template,
                    temperature,
                } => {
                    let prompt = prompt_template.replace("{input}", &value.data.to_string());
                    let request = entelechy_gateway::ModelRequest {
                        model: model.clone(),
                        prompt,
                        temperature: *temperature,
                    };
                    match self.model.infer(&request) {
                        Ok(response) => {
                            value = egress_output(
                                serde_json::json!({ "text": response.text }),
                                &value.meta,
                            );
                        }
                        Err(e) => {
                            return failed(run_id, FailureReason::ModelError(e.to_string()));
                        }
                    }
                }
                CompiledStep::Code { function } => match self.code.get(function) {
                    Some(f) => match f(&value) {
                        Ok(data) => {
                            value = Value {
                                data,
                                meta: value.meta,
                            }
                        }
                        Err(e) => return failed(run_id, FailureReason::CodeError(e)),
                    },
                    None => {
                        return failed(
                            run_id,
                            FailureReason::CodeError(format!("unknown function '{function}'")),
                        )
                    }
                },
                CompiledStep::Verify { checker } => match self.checkers.get(checker) {
                    Some(c) => {
                        if c(&value) {
                            value.meta.verification = entelechy_ir::VerificationState::Verified;
                        } else {
                            return failed(
                                run_id,
                                FailureReason::VerificationFailed(checker.clone()),
                            );
                        }
                    }
                    None => {
                        return failed(
                            run_id,
                            FailureReason::VerificationFailed(format!(
                                "unknown checker '{checker}'"
                            )),
                        )
                    }
                },
                CompiledStep::MemRead { tier, key } => {
                    value = self
                        .memory
                        .read(*tier, key)
                        .cloned()
                        .unwrap_or_else(|| Value::trusted(serde_json::Value::Null));
                }
                CompiledStep::MemWrite { tier, key } => {
                    self.memory.write(*tier, key.clone(), value.clone());
                }
            }
        }
        RunResult {
            status: RunStatus::Succeeded,
            output: Some(value),
            journal: Journal::new(run_id),
        }
    }
}

fn failed(run_id: &str, reason: FailureReason) -> RunResult {
    RunResult {
        status: RunStatus::Failed {
            node_path: "compiled".to_string(),
            reason,
        },
        output: None,
        journal: Journal::new(run_id),
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
    use entelechy_ir::{CodeNode, LlmNode, Node, NodeKind, Program, VerifyNode};

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

    fn register(e: &mut Engine) {
        e.register_code("tag_ok", |v| {
            let mut obj = v.data.clone();
            if let Some(o) = obj.as_object_mut() {
                o.insert("ok".into(), serde_json::json!(true));
            }
            Ok(obj)
        });
        e.register_checker("has_text", |v| v.data.get("text").is_some());
    }

    #[test]
    fn codegen_matches_the_interpreter() {
        let prog = linear_program();
        let plan = CompiledPlan::compile(&prog).unwrap();
        assert_eq!(plan.steps.len(), 3);

        let model = MockModel::new();
        let mut tools = NativeToolGateway::new();

        // Interpreter run.
        let mut e1 = Engine::new(&model, &mut tools);
        register(&mut e1);
        let interp = e1.execute(&prog, Value::trusted(serde_json::json!({})), "r");

        // Codegen run.
        let model2 = MockModel::new();
        let mut tools2 = NativeToolGateway::new();
        let mut e2 = Engine::new(&model2, &mut tools2);
        register(&mut e2);
        let compiled = e2.run_compiled(&plan, Value::trusted(serde_json::json!({})), "r");

        // Interpreter-vs-codegen conformance (Q10).
        assert!(
            observable_equivalent(&interp, &compiled),
            "interp={:?} compiled={:?}",
            interp.status,
            compiled.status
        );
        assert_eq!(compiled.status, RunStatus::Succeeded);
    }

    #[test]
    fn unsupported_shapes_are_refused() {
        use entelechy_ir::{Condition, GateNode};
        let prog = Program::new(
            AuthorityEnvelope::empty(),
            Node::new(
                "g",
                NodeKind::Gate(GateNode {
                    policy: "p".into(),
                    condition: Condition::Always,
                    requires_approval: false,
                }),
            ),
        );
        assert_eq!(
            CompiledPlan::compile(&prog),
            Err(CompileError::Unsupported("gate"))
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
}
