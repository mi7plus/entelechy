# Phase 0 — Prove the thesis

Phase 0 demonstrates that objective-driven evaluation plus bounded optimization
can automatically improve a simple agent **without architecture handcrafting**
(PRD §21). It is a falsifiable research milestone, not a demo.

## Scope (PRD §21)

| Concern | Phase 0 |
|--------|---------|
| Domain | One benchmark: simulated support-triage helpdesk (Q1), ~250 scenarios, ≥100 in holdout |
| Models | One hosted provider (frozen in the StudyPlan) + one local runtime behind an OpenAI-compatible endpoint (Q7) |
| Capabilities | One MCP server or small native tool set |
| Architecture | Single agent only |
| Evaluation | Programmatic checkers + one rubric judge; tune / rolling-validation / holdout splits (default 100/50/100) |
| Optimization | Prompt / model params / tool subset; repair loop |
| Runtime | Interpreter, journal, replay, trace |
| Interface | CLI only |
| Deferred | Multi-agent, generated tools, cluster, plugin registry, Rust codegen, web console |

## Exit criteria (PRD §21 — all required)

- **Improvement:** holdout success beats the baseline by ≥ the pre-registered
  threshold, lower bound of the 95% paired interval above zero.
- **Confirmation:** every accepted change confirmed on rolling-validation first.
- **Safety:** zero structural violations; each behavioral negative goal's upper
  confidence bound ≤ its pre-registered epsilon (§5.6, Q11).
- **Reproducibility:** recorded-effect replay of the accepted candidate
  reproduces observable state exactly.
- **Explainability:** every accepted mutation has a DesignHypothesis + result.
- **Effect safety:** crash injection at each effect-state transition of the
  simulator's write tools produces no silent duplicate effect.
- **Budget / pre-registration / independent reproduction / holdout sealing /
  provider stability / comparator report** — see PRD §21 exit table.

## How the current code maps to Phase 0

Implemented (the runtime substrate the exit criteria depend on):

- **Reproducibility** — `entelechy-runtime` records an append-only journal and
  replays it deterministically, reporting any divergence point. `entelechy demo`
  and `entelechy replay` exercise this. (Exit: *Reproducibility*.)
- **Safety (structural)** — `entelechy-ir::validate` proves IR-I2/I3/I4/I6/I10.
  A refund capability that is forbidden or ungated fails validation. (Exit:
  *Safety*, structural half.)
- **Typed terminal states & budgets** — runs end Succeeded / Failed(typed) /
  BudgetExhausted / ReconciliationRequired; budget exhaustion is a typed
  terminal, not a model failure. (Exit: *Budget*.)
- **Provider identity** — every model call captures provider identity (PV-1),
  the substrate for the *Provider stability* fingerprint checks.
- **Evaluation contract, power and the holdout gate** — `entelechy-eval`
  provides the EvalContract with constraint classes (§5.6), per-split power
  warnings (EV-15), the paired-bootstrap comparison used by the exit rule
  (§9.4/§21), the negative-goal upper-bound test (§5.6), and the audited,
  budgeted holdout gate with the identity firewall (EV-14/EL-1/§9.6). (Exit:
  *Improvement*, *Safety* behavioral half, *Holdout sealing*.)

Not yet implemented (later Phase 0 / Phase 2 per §21.2):

- Signed **StudyPlan** artifact and candidate accounting (`entelechy-bench`,
  `entelechy-search`).
- **Task synthesis** and near-duplicate contamination detection (Q18); Phase 0
  tasks are hand-authored seeds (Q2).
- **Design Repair Engine** and DesignHypothesis lifecycle (`entelechy-design`).
- Crash-injection conformance for the simulator's write tools (RS-1, §8.5).
- The hand-authored GoalSpec / EvalContract / seed tasks (Phase 0 is hand
  authored; the Objective Compiler arrives in Phase 2).

## Phase 0 effect journal (PRD §21)

Phase 0 implements write-ahead intent records and idempotency keys for the
simulated helpdesk's write tools only. The full effect state machine (§8.5),
reconciliation and compensation for real external systems are Phase 1/4 and
*refine*, not replace, the Phase 0 format. The current `ToolEffect` journal entry
already records capability, operation key, request hash and commit status.
