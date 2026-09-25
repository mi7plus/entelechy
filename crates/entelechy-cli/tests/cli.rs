//! Integration smoke tests for the `entelechy` CLI (PRD 16.1).
//!
//! These run the built binary end to end and assert on exit status and output,
//! covering the commands that a change could silently break.

use std::process::Command;

fn entelechy() -> Command {
    Command::new(env!("CARGO_BIN_EXE_entelechy"))
}

fn run(args: &[&str]) -> (bool, String) {
    let out = entelechy()
        .args(args)
        .output()
        .expect("failed to run entelechy binary");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn requirements_lists_the_registry() {
    let (ok, out) = run(&["requirements"]);
    assert!(ok, "requirements failed: {out}");
    assert!(out.contains("27 entries"), "unexpected output: {out}");
    assert!(out.contains("IR-I3") || out.contains("RS-1") || out.contains("AC-1"));
}

#[test]
fn eval_reports_contract_power_and_gate() {
    let (ok, out) = run(&["eval"]);
    assert!(ok, "eval failed: {out}");
    assert!(out.contains("EvalContract"), "no contract line: {out}");
    // The default 100-task holdout can't detect the 10pp target -> power warning.
    assert!(out.contains("Power"), "no power section: {out}");
    // The firewall refuses the design/search plane.
    assert!(out.contains("Firewall") || out.contains("refused"));
}

#[test]
fn study_runs_the_full_loop() {
    let (ok, out) = run(&["study"]);
    assert!(ok, "study failed: {out}");
    assert!(out.contains("Baseline"), "no baseline: {out}");
    assert!(
        out.contains("Rolling validation"),
        "no rolling validation: {out}"
    );
    assert!(out.contains("Holdout gate"), "no holdout gate: {out}");
}

#[test]
fn demo_executes_journals_and_replays() {
    let dir = std::env::temp_dir().join(format!("entelechy-cli-demo-{}", std::process::id()));
    let dir_s = dir.to_string_lossy().into_owned();
    let (ok, out) = run(&["demo", "--out", &dir_s]);
    assert!(ok, "demo failed: {out}");
    assert!(out.contains("Replay:"), "no replay step: {out}");
    assert!(
        out.contains("conformance"),
        "no effect-safety conformance: {out}"
    );
    // Artifacts were written.
    assert!(dir.join("design.json").exists(), "design.json not written");
    assert!(
        dir.join("journal.json").exists(),
        "journal.json not written"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diff_shows_the_repaired_candidate() {
    let (ok, out) = run(&["diff"]);
    assert!(ok, "diff failed: {out}");
    assert!(
        out.contains("reply_check"),
        "diff should add reply_check: {out}"
    );
}

#[test]
fn study_out_writes_design_and_report() {
    let dir = std::env::temp_dir().join(format!("entelechy-study-{}", std::process::id()));
    let dir_s = dir.to_string_lossy().into_owned();
    let (ok, out) = run(&["study", "--out", &dir_s]);
    assert!(ok, "study --out failed: {out}");
    let design = dir.join("design.json");
    let report = dir.join("study-report.json");
    assert!(design.exists(), "design.json not written");
    assert!(report.exists(), "study-report.json not written");
    // The report is valid JSON recording the best content address and holdout.
    let report_text = std::fs::read_to_string(&report).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert!(parsed["best_id"].as_str().unwrap().starts_with("sha256:"));
    assert!(parsed["holdout"]["result"].is_string());
    assert!(!parsed["hypotheses"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn committee_runs_and_synthesizes() {
    let (ok, out) = run(&["committee", "how do I reset my password?"]);
    assert!(ok, "committee failed: {out}");
    // Map/reduce: a coordinator reduce step synthesizes the committee's outputs.
    assert!(out.contains("coordinator"), "no coordinator step: {out}");
    assert!(
        out.contains("Synthesized answer"),
        "no synthesized answer: {out}"
    );
}

#[test]
fn unknown_command_fails() {
    let (ok, _out) = run(&["definitely-not-a-command"]);
    assert!(!ok, "unknown command should exit non-zero");
}
