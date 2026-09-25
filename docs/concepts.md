# Concepts

A short tour of Entelechy's model. The authoritative source is
`Entelechy_PRD_v11_FINAL.docx`; this guide orients you before you read it.

## The thesis

Agentic-system construction is treated as **evidence-driven synthesis of typed
programs under explicit authority, budget and safety constraints**. You specify an
objective, its constraints, the authority it may exercise, and the evidence you
require. **Architecture is a search result, not an input** (§1).

## The lifecycle

```
Objective → GoalSpec → CapabilityGraph → EvalContract → EvalSuite
         → Design → Improve loop → Candidate set → Assurance → Release → Operations
```

Every transition consumes immutable, content-addressed artifacts and emits new
ones; a stage may reject inputs whose invariants don't hold (§5.1).

## Core objects

- **GoalSpec** — the machine-checkable contract: success criteria, negative goals
  (classified structural/behavioral/operational), authority, budgets, selection
  policy, epsilon/delta (§5.3, §5.6, §5.7).
- **AuthorityEnvelope** — first-class authority. Designs cannot widen it;
  delegation only narrows it (A1–A3, §5.4).
- **IR** — the typed program: `Llm`, `Tool`, `Code`, `Seq`, `Par`, `Map`,
  `Branch`, `Loop`, `Delegate`, `Verify`, `Mem`, `Gate`, `Human`. Values carry
  taint and confidentiality labels (§7).
- **EvalContract / holdout** — evaluation is a governed artifact; the holdout is
  reachable only through an audited, budgeted gate (§9, EV-14).
- **DesignHypothesis** — every design change is a hypothesis with evidence, an
  expected effect and an experiment (§11.2).
- **SafetyCase / Release** — claims mapped to evidence; promotion climbs the
  L0–L5 maturity ladder through explicit gates (§14).

## Principles that shape the code

1. Human authority at explicit gates — the system proposes, never self-grants.
2. Hard constraints define feasibility — they are gates, not scalar penalties.
3. Evaluation is part of the specification.
4. Complexity is earned — start with the simplest viable design.
5. Every design change is a hypothesis backed by evidence.
6. Everything is an artifact with lineage.
7. Every external effect is journaled; replay uses recorded effects.
8. Least privilege by construction.

## How constraints are enforced (not just described)

- **Structural** constraints are proven by static IR analysis (`entelechy-ir`)
  and enforced at the effect boundary by the policy engine (`entelechy-policy`).
- **Behavioral** constraints are measured: a violation-rate upper bound must sit
  below epsilon with confidence 1−delta (§5.6, `entelechy-eval`).
- **Operational** constraints are runtime caps in the gateways.

## Where things live

See [ARCHITECTURE.md](../ARCHITECTURE.md) for the crate-to-PRD map and what is
implemented. Start with `entelechy-ir`, then `entelechy-runtime`, then
`entelechy-eval`.
