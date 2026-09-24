//! Entelechy CLI (PRD 16.1). Local-first workflow.
//!
//! Initial command surface from PRD 16.1: init, objective, capability, eval,
//! design, study, trace, replay, diff, assure, release, serve. Commands that a
//! later phase delivers (PRD 21.2) report the phase that will implement them
//! rather than failing silently.

mod demo;
mod release;
mod study;

use clap::{Parser, Subcommand};

/// Entelechy: objective-driven agentic systems synthesis (PRD v11).
#[derive(Parser)]
#[command(name = "entelechy", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new Entelechy workspace in the current directory.
    Init,
    /// Compile an objective into a GoalSpec (Phase 2).
    Objective,
    /// Discover capabilities and build a CapabilityGraph (Phase 1/2).
    Capability,
    /// Review or generate the EvalContract (Phase 2).
    Eval,
    /// Inspect or edit a Design IR.
    Design {
        /// Path to a Design IR JSON file to validate.
        #[arg(long)]
        validate: Option<String>,
    },
    /// Run an optimization study (Phase 2+).
    Study,
    /// Explore a run's trace (Phase 1+).
    Trace,
    /// Replay a recorded journal against a Design IR (PRD 8.2).
    Replay {
        /// Path to a Design IR JSON file.
        #[arg(long)]
        design: String,
        /// Path to a recorded journal JSON file.
        #[arg(long)]
        journal: String,
    },
    /// Diff two Design IRs (PRD 16.2). With no paths, diffs the demo baseline
    /// against its repaired candidate.
    Diff {
        /// Old design IR JSON.
        #[arg(long)]
        old: Option<String>,
        /// New design IR JSON.
        #[arg(long)]
        new: Option<String>,
    },
    /// Run the assurance compiler (Phase 4).
    Assure,
    /// Manage releases (Phase 4).
    Release,
    /// Serve the local HTTP API (loopback only, dev token).
    Serve {
        /// Port to bind on 127.0.0.1 (0 picks an ephemeral port).
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Run one inference against a self-hosted OpenAI-compatible model (Q7).
    /// Requires the `openai` feature: `cargo run -p entelechy-cli --features openai`.
    #[cfg(feature = "openai")]
    Infer {
        /// Endpoint base URL (Ollama 11434, vLLM 8000, llama.cpp 8080, LM Studio 1234).
        #[arg(long, default_value = "http://localhost:11434/v1")]
        base_url: String,
        /// Model name as the server knows it (e.g. `llama3.1`).
        #[arg(long)]
        model: String,
        /// Prompt to send.
        #[arg(long, default_value = "Say hello in exactly five words.")]
        prompt: String,
        /// Optional bearer token (vLLM/LocalAI); omit for Ollama.
        #[arg(long)]
        api_key: Option<String>,
    },
    /// List the normative requirement registry (PRD 17.6).
    Requirements,
    /// Run the bundled end-to-end demo: execute, journal and replay.
    Demo {
        /// Directory to write the demo design + journal into.
        #[arg(long, default_value = "entelechy-demo")]
        out: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init => cmd_init(),
        Command::Requirements => cmd_requirements(),
        #[cfg(feature = "openai")]
        Command::Infer {
            base_url,
            model,
            prompt,
            api_key,
        } => cmd_infer(base_url, model, prompt, api_key),
        Command::Design { validate } => cmd_design(validate),
        Command::Demo { out } => cmd_demo(&out),
        Command::Replay { design, journal } => cmd_replay(&design, &journal),
        Command::Objective => cmd_objective(),
        Command::Capability => cmd_capability(),
        Command::Eval => cmd_eval(),
        Command::Study => study::run(),
        Command::Trace => cmd_trace(),
        Command::Diff { old, new } => cmd_diff(old, new),
        Command::Assure => release::cmd_assure(),
        Command::Release => release::cmd_release(),
        Command::Serve { port } => cmd_serve(port),
    }
}

fn cmd_init() -> anyhow::Result<()> {
    println!("Entelechy local workspace.");
    println!("Deployment profile: local (single security domain, no HA claim) — PRD 17.1.");
    println!("Next: `entelechy demo` runs the bundled execute/journal/replay slice.");
    Ok(())
}

fn cmd_diff(old: Option<String>, new: Option<String>) -> anyhow::Result<()> {
    use entelechy_design::{apply, diff_programs, has_pinned_conflict, ChangeKind, EditOp};

    let (old_prog, new_prog) = match (old, new) {
        (Some(o), Some(n)) => {
            let op: entelechy_ir::Program = serde_json::from_str(&std::fs::read_to_string(o)?)?;
            let np: entelechy_ir::Program = serde_json::from_str(&std::fs::read_to_string(n)?)?;
            (op, np)
        }
        _ => {
            // Demonstrate with the baseline vs. its repaired candidate (adds a
            // reply_check verification step, PRD Appendix C).
            let base = demo::demo_program();
            let candidate = apply(
                &base,
                &EditOp::AddVerify {
                    id: "reply_check".into(),
                    checker: "reply_supported".into(),
                },
            )?;
            println!(
                "(no paths given; diffing the demo baseline against its repaired candidate)\n"
            );
            (base, candidate)
        }
    };

    let diffs = diff_programs(&old_prog, &new_prog);
    for d in &diffs {
        if d.change == ChangeKind::Unchanged {
            continue;
        }
        let mark = match d.change {
            ChangeKind::Added => "+",
            ChangeKind::Removed => "-",
            ChangeKind::Changed => "~",
            ChangeKind::Unchanged => " ",
        };
        let pin = if d.pinned { " [pinned]" } else { "" };
        println!("  {mark} {}{pin}: {}", d.id, d.detail);
    }
    let changes = diffs
        .iter()
        .filter(|d| d.change != ChangeKind::Unchanged)
        .count();
    println!("\n{changes} changed node(s).");
    if has_pinned_conflict(&diffs) {
        println!("WARNING: a pinned node changed — a merge must not overwrite it (IR-I7).");
    }
    Ok(())
}

#[cfg(feature = "openai")]
fn cmd_infer(
    base_url: String,
    model: String,
    prompt: String,
    api_key: Option<String>,
) -> anyhow::Result<()> {
    use entelechy_gateway::{ModelGateway, ModelRequest, OpenAiGateway};

    let gateway = OpenAiGateway::new(&base_url, api_key);
    let req = ModelRequest {
        model,
        prompt,
        temperature: 0.0,
    };
    println!("Calling {base_url} (self-hosted OpenAI-compatible)...");
    match gateway.infer(&req) {
        Ok(resp) => {
            println!("\n{}\n", resp.text);
            let id = resp.identity;
            println!(
                "[provider={} model={} endpoint={} revision={}]",
                id.provider,
                id.advertised_model,
                id.endpoint,
                id.revision.as_deref().unwrap_or("-")
            );
        }
        Err(e) => {
            println!("inference failed: {e}");
            println!(
                "Is a model server running? e.g. `ollama serve` then `ollama pull {}`.",
                req_model_hint()
            );
        }
    }
    Ok(())
}

#[cfg(feature = "openai")]
fn req_model_hint() -> &'static str {
    "llama3.1"
}

fn cmd_serve(port: u16) -> anyhow::Result<()> {
    use entelechy_identity::{Principal, PrincipalKind};
    use entelechy_protocol::{ProtocolVersion, VersionRange};
    use entelechy_server::{ApiServer, HttpServer};

    let mut api = ApiServer::new(VersionRange::new(
        ProtocolVersion::new(1, 0),
        ProtocolVersion::new(1, 0),
    ));
    // A couple of read-only operations any authenticated principal may call.
    api.register(
        "status",
        |_p| true,
        |p, _payload| Ok(serde_json::json!({ "status": "ok", "principal": p.id })),
    );
    api.register(
        "requirements",
        |_p| true,
        |_p, _payload| {
            Ok(serde_json::json!({ "requirement_count": entelechy_contracts::REGISTRY.len() }))
        },
    );

    let mut server = HttpServer::new(api);
    // Loopback-only development token (PRD Q30); server modes use OIDC/mTLS.
    let token = "local-dev-token";
    server.add_dev_token(token, Principal::new("local", PrincipalKind::Operator));

    let listener = HttpServer::bind_local(port)?;
    let addr = listener.local_addr()?;
    println!("Entelechy API serving on http://{addr} (loopback only, PRD 16.2/Q30).");
    println!("Dev token: {token}");
    println!(
        "Try: curl -s -XPOST http://{addr}/v1/status -H 'Authorization: Bearer {token}' -H 'X-Protocol-Version: 1.0' -d '{{}}'"
    );
    println!("Ctrl-C to stop.");
    server.serve(&listener)?;
    Ok(())
}

fn cmd_capability() -> anyhow::Result<()> {
    use entelechy_capability::{Capability, CapabilityGraph, RequiredCapability, SourceKind};
    use entelechy_ir::{AttestationLevel, EffectClass, EffectMetadata};

    let now = 1_000_000u64;
    let mut graph = CapabilityGraph::new();
    // An operator-attested native write tool.
    graph.add(Capability {
        name: "draft_reply".into(),
        source: SourceKind::Native,
        schema: serde_json::json!({"channel": "string"}),
        declared_effect: EffectMetadata {
            class: EffectClass::Write,
            idempotent: true,
            reversible: true,
            dry_run_supported: true,
            read_back_supported: true,
            attestation: AttestationLevel::OperatorAttested,
            operation_key_namespace: "helpdesk.draft".into(),
        },
        authority_scope: vec!["tenant".into()],
        latency_ms: Some(40),
        cost_minor: Some(0),
        reliability: Some(0.99),
        attested_at: Some(now),
        attested_version: Some("v1".into()),
        version: "v1".into(),
    });
    // An unattested MCP write capability (CD-7 / Q13).
    graph.add(Capability {
        name: "create_ticket".into(),
        source: SourceKind::Mcp,
        schema: serde_json::json!({}),
        declared_effect: EffectMetadata {
            class: EffectClass::Write,
            idempotent: false,
            reversible: false,
            dry_run_supported: false,
            read_back_supported: false,
            attestation: AttestationLevel::UntrustedHint,
            operation_key_namespace: "mcp.create_ticket".into(),
        },
        authority_scope: vec![],
        latency_ms: None,
        cost_minor: None,
        reliability: None,
        attested_at: None,
        attested_version: None,
        version: "0.3".into(),
    });

    println!("CapabilityGraph ({} capabilities):", graph.iter().count());
    for cap in graph.iter() {
        let eff = cap.effective_effect(now);
        println!(
            "  {} [{:?}] declared {:?} -> effective {:?}; attested: {}; write-authority: {}",
            cap.name,
            cap.source,
            cap.declared_effect.class,
            eff.class,
            cap.is_attested_at(now),
            cap.may_have_write_authority(now),
        );
    }

    // Compare to the objective's required capabilities (CD-4).
    let required = vec![
        RequiredCapability {
            name: "draft_reply".into(),
            needs_write: true,
        },
        RequiredCapability {
            name: "create_ticket".into(),
            needs_write: true,
        },
        RequiredCapability {
            name: "issue_refund".into(),
            needs_write: true,
        },
    ];
    let gaps = graph.gaps(&required, now);
    println!("\nCapability gaps (CD-4): {}", gaps.len());
    for g in &gaps {
        println!("  {} — {:?}", g.name, g.reason);
    }
    println!("\nUnattested MCP capabilities get no write authority and default to external+irreversible (CD-7/Q13).");
    Ok(())
}

fn cmd_trace() -> anyhow::Result<()> {
    // Execute the demo and render its journal as a trace (OP-1 / PRD 16.2).
    let (result, _decisions) = demo::run_demo("trace-run-1");
    println!(
        "Trace for run '{}' — {}",
        result.journal.run_id,
        demo::status_line(&result)
    );
    for entry in &result.journal.entries {
        let kind = match &entry.event {
            entelechy_runtime::JournalEvent::ModelCall { request, .. } => {
                format!("model-call model={}", request.model)
            }
            entelechy_runtime::JournalEvent::ToolEffect {
                capability, commit, ..
            } => {
                format!("tool-effect capability={capability} commit={commit:?}")
            }
            entelechy_runtime::JournalEvent::PolicyDecision { policy, allowed } => {
                format!("policy-decision policy={policy} allowed={allowed}")
            }
        };
        println!("  [{}] {} :: {}", entry.seq, entry.node_path, kind);
    }
    Ok(())
}

fn cmd_objective() -> anyhow::Result<()> {
    use entelechy_eval::{ConstraintClass, NegativeGoal, RiskClass};
    use entelechy_objective::{
        compile_eval_contract, run_interview, Assumption, BudgetEnvelope, ClarificationQuestion,
        GoalSpec, Provenanced, SelectionPolicy,
    };

    // A bounded clarification interview (OC-3): rank by expected impact, stop at
    // the question budget, defer the rest as assumptions.
    let questions = vec![
        ClarificationQuestion {
            text: "Which ticket categories are in scope?".into(),
            expected_impact: 0.9,
        },
        ClarificationQuestion {
            text: "Is a formal tone required?".into(),
            expected_impact: 0.3,
        },
        ClarificationQuestion {
            text: "What is the escalation path for billing?".into(),
            expected_impact: 0.7,
        },
    ];
    let interview = run_interview(questions, 2);
    println!(
        "Clarification interview (OC-3): asking {} of {} questions.",
        interview.to_ask.len(),
        interview.to_ask.len() + interview.deferred_assumptions.len()
    );
    for q in &interview.to_ask {
        println!("  ask: {} (impact {:.1})", q.text, q.expected_impact);
    }

    // Assumptions: the deferred questions become assumptions, each linked to a
    // falsifying evaluation task (OC-4) so sign-off can proceed.
    let mut assumptions: Vec<Assumption> = interview
        .deferred_assumptions
        .into_iter()
        .enumerate()
        .map(|(i, mut a)| {
            a.falsifying_task = Some(format!("assumption-task-{i}"));
            a
        })
        .collect();
    assumptions.push(Assumption {
        text: "tickets are in English".into(),
        falsifying_task: Some("lang-detect-task".into()),
    });

    let mut authority = entelechy_ir::AuthorityEnvelope::empty();
    authority.capabilities.insert("draft_reply".into());
    authority.forbidden_capabilities.insert("refund".into());

    let goalspec = GoalSpec {
        success_criteria: vec![Provenanced::stated("resolve tier-1 support tickets".into())],
        negative_goals: vec![
            NegativeGoal {
                name: "no_refund".into(),
                class: ConstraintClass::Structural,
                risk: RiskClass::Critical,
                epsilon: None,
                delta: 0.05,
            },
            NegativeGoal {
                name: "no_cross_customer_disclosure".into(),
                class: ConstraintClass::Behavioral,
                risk: RiskClass::High,
                epsilon: None,
                delta: 0.05,
            },
        ],
        authority,
        budget: Provenanced::defaulted(BudgetEnvelope {
            money_minor: 60_000,
            tokens: 1_000_000,
            wall_secs: 3_600,
            steps: 100,
        }),
        selection_policy: SelectionPolicy::ConstrainedOptimization {
            objective: "minimize:cost".into(),
            constraints: vec![],
        },
        assumptions,
        data_classification: Provenanced::stated("internal".into()),
        on_behalf_of_user: true,
        value_estimate_minor: Provenanced::inferred(500_000),
    };

    println!(
        "\nGoalSpec: {} criteria, {} negative goals, ledger valid: {}.",
        goalspec.success_criteria.len(),
        goalspec.negative_goal_count(),
        goalspec.assumption_ledger_valid()
    );

    // Compile the EvalContract from the GoalSpec (EV-2) and check power (EV-15).
    let contract = compile_eval_contract(&goalspec);
    let warnings = contract.power_warnings();
    println!(
        "EvalContract compiled (EV-2): {} criteria, {} negative goals; {} power warning(s).",
        contract.criteria.len(),
        contract.negative_goals.len(),
        warnings.len()
    );

    // Sign off, producing an immutable content-addressed GoalSpec (OC-8).
    match goalspec.sign_off() {
        Ok(signed) => println!("Signed GoalSpec (OC-8) — commitment {}.", signed.id),
        Err(e) => println!("Sign-off refused: {e}"),
    }
    Ok(())
}

fn cmd_eval() -> anyhow::Result<()> {
    use entelechy_eval::{
        CallerIdentity, ConstraintClass, Difficulty, EvalContract, GateResponse, HoldoutVault,
        NegativeGoal, Plane, Provenance, ReleaseRule, RiskClass, Split, SplitPolicy, Task,
    };

    // The Phase 0 support-triage EvalContract (PRD 21, Q1), hand-authored.
    let contract = EvalContract {
        version: 1,
        criteria: vec!["resolves_tier1_ticket".into()],
        negative_goals: vec![
            NegativeGoal {
                name: "no_refund".into(),
                class: ConstraintClass::Structural,
                risk: RiskClass::Critical,
                epsilon: None,
                delta: 0.05,
            },
            NegativeGoal {
                name: "no_cross_customer_disclosure".into(),
                class: ConstraintClass::Behavioral,
                risk: RiskClass::High,
                epsilon: None,
                delta: 0.05,
            },
        ],
        splits: SplitPolicy::default(), // 100 / 50 / 100 (Q1)
        release: ReleaseRule {
            primary_metric: "task_success".into(),
            target_improvement_pp: 10.0,
            holdout_query_budget: 5,
        },
    };

    println!(
        "EvalContract v{} — primary metric '{}', target +{}pp (Q1 split {}/{}/{}).",
        contract.version,
        contract.release.primary_metric,
        contract.release.target_improvement_pp,
        contract.splits.tune,
        contract.splits.validation,
        contract.splits.holdout
    );

    println!("\nNegative goals:");
    for g in &contract.negative_goals {
        match g.class {
            ConstraintClass::Structural => {
                println!(
                    "  {} [structural] — proven by IR analysis; no epsilon.",
                    g.name
                );
            }
            _ => println!(
                "  {} [{:?}] — upper-bound test, epsilon {:.1}%.",
                g.name,
                g.class,
                g.effective_epsilon() * 100.0
            ),
        }
    }

    // Power check (EV-15 / OC-6).
    let warnings = contract.power_warnings();
    if warnings.is_empty() {
        println!("\nPower: every confirmatory split can detect the target improvement.");
    } else {
        println!("\nPower warnings (EV-15): the target is below the minimum detectable effect:");
        for w in &warnings {
            println!(
                "  {:?}: MDE ~{:.0}pp > target {:.0}pp — enlarge this split or raise the target.",
                w.split, w.mde_pp, w.target_pp
            );
        }
    }

    // Seal a small holdout and exercise the gate (EV-14) + firewall (9.6).
    let holdout: Vec<Task> = (0..contract.splits.holdout)
        .map(|i| Task {
            id: format!("h{i}"),
            input: serde_json::json!({ "ticket": i }),
            environment: "helpdesk".into(),
            checkers: vec!["state".into()],
            tags: vec![],
            difficulty: Difficulty::Medium,
            provenance: Provenance::HumanSeed,
            split: Split::Holdout,
            source_task: None,
        })
        .collect();
    let mut vault = HoldoutVault::seal(&contract, holdout)?;
    println!(
        "\nHoldout sealed: {} tasks (content is not readable — EV-14).",
        vault.len()
    );

    // Firewall: a design/search identity is refused (9.6).
    let design = CallerIdentity {
        id: "search".into(),
        plane: Plane::DesignSearch,
    };
    let refused = vault.gate_query(&design, "cand#tuned", &contract, &|_| false, &|_| true);
    if let GateResponse::Refused { reason } = &refused {
        println!("Firewall: design/search gate query refused — {reason}");
    }

    // Assurance queries the gate: naive baseline (fails all) vs tuned candidate.
    let assurance = CallerIdentity {
        id: "assure".into(),
        plane: Plane::Assurance,
    };
    let resp = vault.gate_query(
        &assurance,
        "cand#tuned",
        &contract,
        &|t| t.input.get("ticket").and_then(|x| x.as_u64()).unwrap_or(0) % 3 == 0, // baseline ~33%
        &|_| true,                                                                 // candidate 100%
    );
    match resp {
        GateResponse::Pass {
            ci_low_pp,
            ci_high_pp,
        } => println!(
            "Gate: PASS — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp (coarse)."
        ),
        GateResponse::Fail {
            ci_low_pp,
            ci_high_pp,
        } => println!("Gate: FAIL — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp."),
        GateResponse::Refused { reason } => println!("Gate: REFUSED — {reason}"),
    }
    println!(
        "Remaining holdout query budget: {}.",
        vault.remaining_budget()
    );
    Ok(())
}

fn cmd_requirements() -> anyhow::Result<()> {
    use entelechy_contracts::REGISTRY;
    println!(
        "Cross-cutting requirement registry (PRD 17.6) — {} entries:\n",
        REGISTRY.len()
    );
    for r in REGISTRY {
        println!(
            "  {:<5} [{:?}, first enforced {:?}]  {} (§{})",
            r.id, r.verification, r.first_enforced, r.summary, r.section
        );
    }
    Ok(())
}

fn cmd_design(validate: Option<String>) -> anyhow::Result<()> {
    let Some(path) = validate else {
        println!("Provide --validate <design.json> to statically check a Design IR (PRD 7.4).");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path)?;
    let program: entelechy_ir::Program = serde_json::from_str(&text)?;
    let catalog = demo::demo_catalog();
    let violations = entelechy_ir::validate(&program, &catalog);
    if violations.is_empty() {
        println!("OK: no invariant violations (checked IR-I2/I3/I4/I6/I10).");
    } else {
        println!("{} invariant violation(s):", violations.len());
        for v in &violations {
            println!("  {} at {}: {}", v.code, v.node_id, v.message);
        }
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_demo(out_dir: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(out_dir)?;

    // 1. Static validation (PRD 7.4).
    let program = demo::demo_program();
    let catalog = demo::demo_catalog();
    let violations = entelechy_ir::validate(&program, &catalog);
    println!(
        "1. Static invariant check: {} violation(s).",
        violations.len()
    );
    for v in &violations {
        println!("   {} at {}: {}", v.code, v.node_id, v.message);
    }

    // 2. Execute, journaling every effect (RK-2) and authorizing effects at the
    //    boundary via the policy engine (PRD 8.3).
    let (result, decisions) = demo::run_demo("demo-run-1");
    println!(
        "2. Execute: {} ({decisions} policy decision(s) recorded at effect boundaries).",
        demo::status_line(&result)
    );

    // 3. Persist design + journal (content-addressed-ready artifacts).
    let design_path = format!("{out_dir}/design.json");
    let journal_path = format!("{out_dir}/journal.json");
    std::fs::write(&design_path, serde_json::to_string_pretty(&program)?)?;
    std::fs::write(
        &journal_path,
        serde_json::to_string_pretty(&result.journal)?,
    )?;
    let design_id = entelechy_artifacts::ArtifactId::of(&program)?;
    println!("3. Wrote {design_path} (id {design_id}) and {journal_path}.");

    // 4. Replay the recorded journal (PRD 8.2, 18).
    let replayed = replay(&program, &result.journal)?;
    println!("4. Replay: {replayed}");

    // 5. Crash-injection conformance for effect safety (RS-1, PRD 8.5).
    let rows = entelechy_effects::crash_injection_conformance();
    let passed = rows.iter().filter(|r| r.ok).count();
    let reconc = rows.iter().filter(|r| r.reconciliation_required).count();
    println!(
        "5. Crash-injection conformance: {passed}/{} transitions with no silent duplicate effect; {reconc} required reconciliation (RS-1).",
        rows.len()
    );

    println!(
        "\nDone. Phase 0 slice: execute -> journal -> replay + effect-safety conformance (PRD 21)."
    );
    Ok(())
}

fn cmd_replay(design_path: &str, journal_path: &str) -> anyhow::Result<()> {
    let program: entelechy_ir::Program =
        serde_json::from_str(&std::fs::read_to_string(design_path)?)?;
    let journal: entelechy_runtime::Journal =
        serde_json::from_str(&std::fs::read_to_string(journal_path)?)?;
    println!("{}", replay(&program, &journal)?);
    Ok(())
}

/// Replay a program against a journal using the demo registries.
fn replay(
    program: &entelechy_ir::Program,
    journal: &entelechy_runtime::Journal,
) -> anyhow::Result<String> {
    use entelechy_gateway::{MockModel, NativeToolGateway};
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = entelechy_runtime::Engine::new(&model, &mut tools);
    engine.register_code("route_ticket", |v| Ok(v.data.clone()));
    engine.register_checker("reply_nonempty", |_| true);
    match engine.replay(
        program,
        entelechy_ir::Value::trusted(serde_json::json!({})),
        journal,
    ) {
        Ok(_) => Ok(format!(
            "reproduced observable state deterministically ({} entries, no divergence)",
            journal.entries.len()
        )),
        Err(mismatch) => Ok(format!("DIVERGENCE: {mismatch}")),
    }
}
