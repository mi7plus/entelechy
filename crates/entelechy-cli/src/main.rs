//! Entelechy CLI (PRD 16.1). Local-first workflow.
//!
//! Initial command surface from PRD 16.1: init, objective, capability, eval,
//! design, study, trace, replay, diff, assure, release, serve. Commands that a
//! later phase delivers (PRD 21.2) report the phase that will implement them
//! rather than failing silently.

mod demo;
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
    /// Diff two Design IRs (Phase 3).
    Diff,
    /// Run the assurance compiler (Phase 4).
    Assure,
    /// Manage releases (Phase 4).
    Release,
    /// Serve the API (Phase 4; entelechy-server).
    Serve,
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
        Command::Design { validate } => cmd_design(validate),
        Command::Demo { out } => cmd_demo(&out),
        Command::Replay { design, journal } => cmd_replay(&design, &journal),
        Command::Objective => planned("objective", "Phase 2 (Objective Compiler, PRD 5.5)"),
        Command::Capability => planned("capability", "Phase 1/2 (capability discovery, PRD 13.1)"),
        Command::Eval => cmd_eval(),
        Command::Study => study::run(),
        Command::Trace => planned("trace", "Phase 1 (trace explorer, PRD 16.2)"),
        Command::Diff => planned("diff", "Phase 3 (IR diff/merge, PRD 16.2)"),
        Command::Assure => planned("assure", "Phase 4 (assurance compiler, PRD 14.1)"),
        Command::Release => planned("release", "Phase 4 (release bundles, PRD 14)"),
        Command::Serve => planned("serve", "Phase 4 (entelechy-server, PRD 16.2)"),
    }
}

fn planned(name: &str, phase: &str) -> anyhow::Result<()> {
    println!("`entelechy {name}` is planned for {phase}.");
    println!("This workspace currently implements the Phase 0/1 core: run `entelechy demo`.");
    Ok(())
}

fn cmd_init() -> anyhow::Result<()> {
    println!("Entelechy local workspace.");
    println!("Deployment profile: local (single security domain, no HA claim) — PRD 17.1.");
    println!("Next: `entelechy demo` runs the bundled execute/journal/replay slice.");
    Ok(())
}

fn cmd_eval() -> anyhow::Result<()> {
    use entelechy_eval::{
        CallerIdentity, ConstraintClass, EvalContract, GateResponse, HoldoutVault, NegativeGoal,
        Plane, ReleaseRule, RiskClass, SplitPolicy, Split, Task, Difficulty, Provenance,
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

    println!("EvalContract v{} — primary metric '{}', target +{}pp (Q1 split {}/{}/{}).",
        contract.version, contract.release.primary_metric, contract.release.target_improvement_pp,
        contract.splits.tune, contract.splits.validation, contract.splits.holdout);

    println!("\nNegative goals:");
    for g in &contract.negative_goals {
        match g.class {
            ConstraintClass::Structural => {
                println!("  {} [structural] — proven by IR analysis; no epsilon.", g.name);
            }
            _ => println!(
                "  {} [{:?}] — upper-bound test, epsilon {:.1}%.",
                g.name, g.class, g.effective_epsilon() * 100.0
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
    println!("\nHoldout sealed: {} tasks (content is not readable — EV-14).", vault.len());

    // Firewall: a design/search identity is refused (9.6).
    let design = CallerIdentity { id: "search".into(), plane: Plane::DesignSearch };
    let refused = vault.gate_query(&design, "cand#tuned", &contract, &|_| false, &|_| true);
    if let GateResponse::Refused { reason } = &refused {
        println!("Firewall: design/search gate query refused — {reason}");
    }

    // Assurance queries the gate: naive baseline (fails all) vs tuned candidate.
    let assurance = CallerIdentity { id: "assure".into(), plane: Plane::Assurance };
    let resp = vault.gate_query(
        &assurance,
        "cand#tuned",
        &contract,
        &|t| t.input.get("ticket").and_then(|x| x.as_u64()).unwrap_or(0) % 3 == 0, // baseline ~33%
        &|_| true,                                                                  // candidate 100%
    );
    match resp {
        GateResponse::Pass { ci_low_pp, ci_high_pp } => println!(
            "Gate: PASS — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp (coarse)."
        ),
        GateResponse::Fail { ci_low_pp, ci_high_pp } => println!(
            "Gate: FAIL — improvement 95% interval [{ci_low_pp:.1}, {ci_high_pp:.1}]pp."
        ),
        GateResponse::Refused { reason } => println!("Gate: REFUSED — {reason}"),
    }
    println!("Remaining holdout query budget: {}.", vault.remaining_budget());
    Ok(())
}

fn cmd_requirements() -> anyhow::Result<()> {
    use entelechy_contracts::REGISTRY;
    println!("Cross-cutting requirement registry (PRD 17.6) — {} entries:\n", REGISTRY.len());
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
    println!("1. Static invariant check: {} violation(s).", violations.len());
    for v in &violations {
        println!("   {} at {}: {}", v.code, v.node_id, v.message);
    }

    // 2. Execute, journaling every effect (RK-2).
    let result = demo::run_demo("demo-run-1");
    println!("2. Execute: {}", demo::status_line(&result));

    // 3. Persist design + journal (content-addressed-ready artifacts).
    let design_path = format!("{out_dir}/design.json");
    let journal_path = format!("{out_dir}/journal.json");
    std::fs::write(&design_path, serde_json::to_string_pretty(&program)?)?;
    std::fs::write(&journal_path, serde_json::to_string_pretty(&result.journal)?)?;
    let design_id = entelechy_artifacts::ArtifactId::of(&program)?;
    println!("3. Wrote {design_path} (id {design_id}) and {journal_path}.");

    // 4. Replay the recorded journal (PRD 8.2, 18).
    let replayed = replay(&program, &result.journal)?;
    println!("4. Replay: {replayed}");

    println!("\nDone. This is the Phase 0 execute -> journal -> replay slice (PRD 21).");
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
fn replay(program: &entelechy_ir::Program, journal: &entelechy_runtime::Journal) -> anyhow::Result<String> {
    use entelechy_gateway::{MockModel, NativeToolGateway};
    let model = MockModel::new();
    let mut tools = NativeToolGateway::new();
    let mut engine = entelechy_runtime::Engine::new(&model, &mut tools);
    engine.register_code("route_ticket", |v| Ok(v.data.clone()));
    engine.register_checker("reply_nonempty", |_| true);
    match engine.replay(program, entelechy_ir::Value::trusted(serde_json::json!({})), journal) {
        Ok(_) => Ok(format!(
            "reproduced observable state deterministically ({} entries, no divergence)",
            journal.entries.len()
        )),
        Err(mismatch) => Ok(format!("DIVERGENCE: {mismatch}")),
    }
}
