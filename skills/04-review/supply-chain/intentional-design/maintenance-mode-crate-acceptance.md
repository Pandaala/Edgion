---
name: maintenance-mode-crate-acceptance
description: Use when evaluating whether to accept a feature-complete / low-activity crate as a new or retained dependency; documents the acceptance criteria derived from the kstring evaluation.
knowledge_type: intentional-design
---

# Maintenance-Mode Crate Acceptance — DESIGN-TRADEOFF

**Established**: 2026-05-15 (derived from `kstring` 2.0.2 evaluation)
**Source analysis**: `skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md`

## Conclusion

A crate that is feature-complete and has low recent commit activity is **acceptable** as a dependency if it meets all four criteria below. Proposals to block adoption solely on the basis of low release frequency are **not accepted** when the four criteria are satisfied.

## Core Rationale

**1. Feature-complete ≠ abandoned**

Some crates reach a stable plateau where no new features are needed. The absence of recent releases does not indicate abandonment — it may mean the crate is doing exactly what it needs to do. Evaluate maintenance state using qualitative signals (response to issues, CI status, RUSTSEC advisory history) rather than commit frequency alone.

**2. Acceptance criteria (all four must hold)**

| Criterion | Threshold | Notes |
|-----------|-----------|-------|
| No RUSTSEC advisories | Zero active advisories against the crate itself | Transitive dep advisories are triaged separately |
| Recognized maintainer | Crate is maintained by a known Rust ecosystem contributor or organization with a track record of responsible disclosure | E.g., crates-io team member, popular framework maintainer |
| License | MIT, Apache-2.0, or dual MIT/Apache-2.0 | Other OSI-approved licenses require explicit sign-off |
| MSRV compatibility | Does not raise effective MSRV above the project's current stable CI floor | Check `rust-version` in `Cargo.toml` and the CI matrix |

**3. `kstring` as the reference case**

`kstring` 2.0.2 was evaluated as a candidate dependency for `Value::Str` SSO. Despite no feature release since 2024-07-25 (~22 months at evaluation time), it satisfied all four criteria:

- Zero RUSTSEC advisories against `kstring` itself.
- Maintainer: Ed Page (Rust libs-team, clap maintainer) — established, responsive track record.
- License: MIT/Apache-2.0 dual.
- MSRV: would have raised the floor to 1.95.0 — this was a **blocker** at evaluation time because it exceeded the project's current stable CI floor.

The MSRV criterion was the deciding factor that blocked adoption (in addition to the headline zero-copy benefit being fictitious — see the source analysis). The maintenance-mode state was accepted as non-blocking.

**Implication**: MSRV compatibility is a hard gate; maintenance-mode state alone is not a blocker when the other three criteria pass.

## Fix Suggestions Not Accepted

- "Reject any crate with no release in the past 12 months" — feature-complete crates are commonly inactive for longer periods without risk.
- "Prefer a more active fork over the upstream" — forking introduces its own supply-chain risk; prefer upstream unless a fork has become the de facto standard.

## Re-evaluation Triggers

Re-examine a previously accepted maintenance-mode crate if:

- A RUSTSEC advisory is published against the crate.
- The maintainer transfers ownership to an unknown party or the repository is archived.
- The MSRV floor the crate imposes becomes incompatible with a future CI matrix change.
- A successor crate emerges that is API-compatible and actively maintained.

## Reference Cases

- `skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md` — full `kstring` evaluation including MSRV analysis, maintainer assessment, and rejection rationale
- `tasks/todo/dsl-value-str-arc-smartstring.md` — original migration proposal (superseded)
