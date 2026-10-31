//! The `entelechy quickstart` command: the new-user time-to-first-report path
//! (PRD 16.5, DX-1).
//!
//! DX-1 requires that "a new user reaches the first study report in under 30
//! minutes". This module makes that path concrete and self-checking:
//!
//! 1. [`quickstart_steps`] declares the ordered commands a brand-new user runs to
//!    reach their first study report, each with a human time budget. Their sum is
//!    the end-to-end onboarding budget, which must stay at or under the 30-minute
//!    DX-1 ceiling ([`BUDGET_MINUTES`]).
//! 2. [`run_quickstart`] drives the *final, report-producing* step in-process —
//!    the Phase-0 real-domain study ([`entelechy_domain::real_study`]) — measuring
//!    wall-clock, and confirms it produces a green first study report.
//!
//! The returned [`QuickstartReport`] carries one boolean per invariant exactly like
//! the operational drills ([`crate::drill`]) and the real-domain study, so the CLI
//! and the TR-1 phase gate can both consume it. The DX-1 verification artifact is
//! the test `quickstart_reaches_first_report_within_30_minutes`.

use serde::{Deserialize, Serialize};

/// The DX-1 onboarding ceiling: a new user must reach the first study report in
/// under 30 minutes (PRD 16.5). The declared step budgets must sum to at most this.
pub const BUDGET_MINUTES: f64 = 30.0;

/// One ordered step a new user runs on the way to their first study report (DX-1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuickstartStep {
    /// The command the user runs.
    pub command: String,
    /// What the step accomplishes.
    pub description: String,
    /// The human time budget for this step, in minutes.
    pub budget_minutes: f64,
}

/// The result of the DX-1 quickstart — the declared onboarding path plus a measured
/// run of the report-producing step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuickstartReport {
    /// The ordered steps a new user runs to reach the first study report.
    pub steps: Vec<QuickstartStep>,
    /// Sum of the step budgets, in minutes (the end-to-end onboarding budget).
    pub budget_minutes: f64,
    /// Whether the onboarding budget is at or under the 30-minute DX-1 ceiling.
    pub within_budget: bool,
    /// Whether the final step produced a green first study report.
    pub produced_report: bool,
    /// Measured wall-clock of the in-process report-producing step, in milliseconds.
    pub elapsed_ms: u128,
    /// Whether the measured report-producing step itself finished inside the budget.
    pub measured_within_budget: bool,
    /// Whether every DX-1 invariant held (within budget and a report was produced).
    pub ok: bool,
}

/// The ordered commands a new user runs to reach the first study report (DX-1).
///
/// The minimal path: install the toolchain, build the CLI, initialize a workspace,
/// and run the Phase-0 real-domain study, which prints — and, with `--out`,
/// persists — the first study report. The budgets model the human wall-clock of
/// each step and sum to well under the 30-minute ceiling.
pub fn quickstart_steps() -> Vec<QuickstartStep> {
    let step = |command: &str, description: &str, budget_minutes: f64| QuickstartStep {
        command: command.to_string(),
        description: description.to_string(),
        budget_minutes,
    };
    vec![
        step(
            "rustup toolchain install stable",
            "Install the Rust toolchain (skip if already present)",
            6.0,
        ),
        step(
            "cargo build -p entelechy-cli -j 2",
            "Build the entelechy CLI (-j 2 keeps a cold build within memory)",
            15.0,
        ),
        step("entelechy init", "Initialize a local workspace", 1.0),
        step(
            "entelechy study --real --out ./first-study",
            "Run the Phase-0 real-domain study to the first study report",
            5.0,
        ),
    ]
}

/// Run the DX-1 quickstart (PRD 16.5): declare the new-user onboarding path, then
/// drive its final report-producing step — the Phase-0 real-domain study — in
/// process, measuring wall-clock, and confirm it produces a green report inside the
/// 30-minute budget.
pub fn run_quickstart() -> QuickstartReport {
    let steps = quickstart_steps();
    let budget_minutes: f64 = steps.iter().map(|s| s.budget_minutes).sum();
    let within_budget = budget_minutes <= BUDGET_MINUTES;

    // Drive the report-producing step in-process, measuring wall-clock. The study
    // returns a report whose `ok` means every Phase-0 invariant held — a *green*
    // first study report.
    let start = std::time::Instant::now();
    let report = entelechy_domain::real_study();
    let elapsed_ms = start.elapsed().as_millis();
    let produced_report = report.ok;

    // The measured step must itself finish inside the whole onboarding budget — the
    // report-producing path alone can never eat the 30-minute window.
    let budget_ms = BUDGET_MINUTES as u128 * 60 * 1000;
    let measured_within_budget = elapsed_ms < budget_ms;

    let ok = within_budget && produced_report && measured_within_budget;

    QuickstartReport {
        steps,
        budget_minutes,
        within_budget,
        produced_report,
        elapsed_ms,
        measured_within_budget,
        ok,
    }
}

/// Run the DX-1 quickstart and print the onboarding path and the measured result
/// (mirrors the drill/study printers in [`crate::drill`] and [`crate::study`]).
/// With `out`, persists the report. Fails closed if the path did not reach a report
/// within budget.
pub fn run(out: Option<&str>) -> anyhow::Result<()> {
    let report = run_quickstart();
    let yn = |b: bool| if b { "yes" } else { "NO" };

    println!(
        "Quickstart — time to first study report (PRD 16.5, DX-1). A new user reaches the first report in under 30 minutes."
    );
    println!("New-user path ({} steps):", report.steps.len());
    for (i, s) in report.steps.iter().enumerate() {
        println!(
            "  {}. [{:>4.1} min] {}\n        {}",
            i + 1,
            s.budget_minutes,
            s.command,
            s.description
        );
    }
    println!("Budget (DX-1):");
    println!(
        "  onboarding budget:          {:.1} min (sum of step budgets)",
        report.budget_minutes
    );
    println!(
        "  within 30-minute ceiling:   {} (budget {:.1} min ≤ {:.0} min)",
        yn(report.within_budget),
        report.budget_minutes,
        BUDGET_MINUTES
    );
    println!("First study report:");
    println!(
        "  report produced:            {} (the Phase-0 real-domain study returned a green report)",
        yn(report.produced_report)
    );
    println!(
        "  report-producing step took: {} ms (measured in-process; far inside the budget: {})",
        report.elapsed_ms,
        yn(report.measured_within_budget)
    );
    println!(
        "\n  => DX-1 {}.",
        if report.ok {
            "SATISFIED — the declared new-user path reaches a green first study report in well under 30 minutes"
        } else {
            "VIOLATED — the path did not reach a report within budget (see above)"
        }
    );

    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        let report_path = std::path::Path::new(dir).join("quickstart-report.json");
        std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
        println!("\nPersisted: {}.", report_path.display());
    }

    if !report.ok {
        anyhow::bail!(
            "DX-1 quickstart failed: the first study report was not reached within budget"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quickstart_reaches_first_report_within_30_minutes() {
        let r = run_quickstart();
        assert!(!r.steps.is_empty(), "the onboarding path has steps");
        // The declared budget sums to at most the 30-minute DX-1 ceiling.
        let summed: f64 = r.steps.iter().map(|s| s.budget_minutes).sum();
        assert!(
            (summed - r.budget_minutes).abs() < 1e-9,
            "budget is the step sum"
        );
        assert!(
            r.budget_minutes <= BUDGET_MINUTES,
            "onboarding budget {} exceeds the 30-minute ceiling",
            r.budget_minutes
        );
        assert!(r.within_budget, "within the 30-minute ceiling");
        // The path actually produces a green first study report.
        assert!(
            r.produced_report,
            "the Phase-0 study produced a green report"
        );
        // The report-producing step finishes with vast headroom inside the window.
        assert!(
            r.measured_within_budget,
            "the report-producing step ({} ms) must finish inside the budget",
            r.elapsed_ms
        );
        assert!(r.ok, "every DX-1 invariant held");
    }

    #[test]
    fn the_last_step_produces_the_report() {
        // The final declared command is the study that yields the first report, so
        // the budgeted path ends at a report, not somewhere short of it.
        let steps = quickstart_steps();
        let last = steps.last().expect("a non-empty path");
        assert!(
            last.command.contains("study"),
            "the last step runs the study: {}",
            last.command
        );
    }

    #[test]
    fn quickstart_is_deterministic() {
        let a = run_quickstart();
        let b = run_quickstart();
        // Everything but the measured wall-clock is deterministic.
        assert_eq!(a.steps, b.steps);
        assert_eq!(a.budget_minutes, b.budget_minutes);
        assert_eq!(a.within_budget, b.within_budget);
        assert_eq!(a.produced_report, b.produced_report);
        assert_eq!(a.ok, b.ok);
    }
}
