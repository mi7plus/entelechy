# Security Policy

Entelechy is security-critical infrastructure: it enforces authority, effect and
data-flow invariants for generated agentic systems. We take vulnerabilities
seriously and follow coordinated disclosure (PRD §16.5, Q26).

## Reporting a vulnerability

**Do not open a public issue for security reports.**

Report privately through GitHub's private vulnerability reporting:
**Security → Report a vulnerability** on the repository
(<https://github.com/mi7plus/entelechy/security/advisories/new>).

Please include:

- affected component/crate and version or commit,
- a description and, where possible, a reproduction or proof of concept,
- the impact you expect (e.g. authority bypass, sandbox escape, secret leakage,
  cross-tenant access, replay/effect duplication).

## Our commitment

- A security team of at least two maintainers reviews reports (Q26).
- We acknowledge receipt within **5 business days**.
- We follow **90-day coordinated disclosure** (Q26): we aim to ship a fix and a
  published advisory within 90 days, coordinating a disclosure date with you.
- In a security emergency we may shorten a deprecation/removal window to as little
  as 14 days, or remove an interface immediately when exploitation is active
  (PRD §16.6, Q29), documented in the advisory.

## Scope — what we consider a vulnerability

Anything that breaks a stated safety/authority contract, including:

- exercising authority outside the `AuthorityEnvelope`, or a generated design
  widening its own authority (Invariants A1–A3, §5.4);
- a tainted value reaching a privileged effect without a Gate (IR-I3), or a
  confidentiality label leaving its approved provider/scope (IR-I9, §7.5);
- sandbox escape or a plugin obtaining ambient authority (§16.3);
- secret material reaching prompts or ordinary traces (SecretGateway, §8.3);
- cross-tenant reads via content hashes, caches or indexes (§17.2);
- holdout task-content leakage to design/search identities (§9.6, EV-14);
- silent duplicate consequential effects on crash/retry (§8.5, RS-1);
- rewriting the tamper-evident audit log or bypassing approval hash-binding
  (§17.2, §14.5).

## Supported versions

Until 1.0 the `main` branch is the supported line; fixes land there first.
