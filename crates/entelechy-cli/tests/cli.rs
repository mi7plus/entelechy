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
    let audit = dir.join("audit-log.json");
    assert!(design.exists(), "design.json not written");
    assert!(report.exists(), "study-report.json not written");
    assert!(audit.exists(), "audit-log.json not written");
    // The report is valid JSON recording the best content address and holdout.
    let report_text = std::fs::read_to_string(&report).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&report_text).unwrap();
    assert!(parsed["best_id"].as_str().unwrap().starts_with("sha256:"));
    assert!(parsed["holdout"]["result"].is_string());
    assert!(!parsed["hypotheses"].as_array().unwrap().is_empty());
    // Lineage: a DerivedFrom chain from baseline to best (one edge per accepted
    // step). from/to are ArtifactId structs so each edge round-trips back into a
    // typed LineageEdge.
    let edges = parsed["lineage"].as_array().unwrap();
    assert!(!edges.is_empty(), "expected a lineage chain");
    assert!(edges.iter().all(|e| e["kind"] == "derived-from"));
    let digest = |id: &serde_json::Value| {
        id.as_str()
            .unwrap()
            .strip_prefix("sha256:")
            .unwrap()
            .to_string()
    };
    // The chain starts at the baseline and ends at the best design.
    assert_eq!(edges[0]["from"]["digest"], digest(&parsed["baseline_id"]));
    assert_eq!(
        edges.last().unwrap()["to"]["digest"],
        digest(&parsed["best_id"])
    );
    // It is a connected chain: each edge's `to` is the next edge's `from`.
    for pair in edges.windows(2) {
        assert_eq!(pair[0]["to"], pair[1]["from"]);
    }

    // Audit log: hash-chained entries, and the report commits to the chain head.
    let audit_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&audit).unwrap()).unwrap();
    let entries = audit_json["entries"].as_array().unwrap();
    assert!(!entries.is_empty(), "audit log has no entries");
    // The report's audit_head equals the last entry's hash (commits to the chain).
    assert_eq!(
        parsed["audit_head"],
        entries.last().unwrap()["entry_hash"],
        "report audit_head must match the audit chain head"
    );
    // The chain links: each entry's prev_hash is the previous entry's hash.
    for pair in entries.windows(2) {
        assert_eq!(pair[1]["prev_hash"], pair[0]["entry_hash"]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_verify_passes_clean_and_detects_tampering() {
    let dir = std::env::temp_dir().join(format!("entelechy-audit-{}", std::process::id()));
    let dir_s = dir.to_string_lossy().into_owned();
    let (ok, out) = run(&["study", "--out", &dir_s]);
    assert!(ok, "study --out failed: {out}");

    // A clean bundle verifies.
    let (ok, out) = run(&["audit", &dir_s]);
    assert!(ok, "audit should pass on an untampered log: {out}");
    assert!(out.contains("VALID"), "expected VALID: {out}");

    // A previously-held external anchor (the current chain head) is still present.
    let head = out
        .lines()
        .find_map(|l| {
            l.rsplit("head: ")
                .next()
                .filter(|h| h.starts_with("sha256:"))
        })
        .expect("audit output should print the chain head")
        .to_string();
    let (ok, out) = run(&["audit", &dir_s, "--anchor", &head]);
    assert!(ok, "held anchor should verify: {out}");
    assert!(out.contains("found in the chain history"), "{out}");
    // A bogus anchor (a full rewrite would produce one) is rejected.
    let (ok, _) = run(&["audit", &dir_s, "--anchor", "sha256:deadbeef"]);
    assert!(!ok, "an unknown anchor must fail (full-rewrite detection)");

    // Tampering with a recorded decision breaks the chain and fails verification.
    let log = dir.join("audit-log.json");
    let text = std::fs::read_to_string(&log).unwrap();
    let tampered = text.replace("result Accepted", "result Rejected");
    assert_ne!(text, tampered, "expected to alter a recorded decision");
    std::fs::write(&log, tampered).unwrap();
    let (ok, out) = run(&["audit", &dir_s]);
    assert!(!ok, "audit should fail on a tampered log: {out}");
    assert!(out.contains("BROKEN"), "expected BROKEN: {out}");

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
fn committee_out_persists_a_replayable_run() {
    let dir = std::env::temp_dir().join(format!("entelechy-committee-{}", std::process::id()));
    let dir_s = dir.to_string_lossy().into_owned();
    let (ok, out) = run(&[
        "committee",
        "reset password",
        "--agents",
        "3",
        "--out",
        &dir_s,
    ]);
    assert!(ok, "committee --out failed: {out}");
    let design = dir.join("design.json");
    let journal = dir.join("journal.json");
    assert!(design.exists() && journal.exists(), "artifacts not written");
    // The persisted committee run replays deterministically (PRD 8.2).
    let (ok, out) = run(&[
        "replay",
        "--design",
        &design.to_string_lossy(),
        "--journal",
        &journal.to_string_lossy(),
    ]);
    assert!(ok, "replay failed: {out}");
    assert!(
        out.contains("no divergence"),
        "expected clean replay: {out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_command_fails() {
    let (ok, _out) = run(&["definitely-not-a-command"]);
    assert!(!ok, "unknown command should exit non-zero");
}
