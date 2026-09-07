---
name: fully-review
description: Use when running or preparing a global codebase review pass that produces tasks/review-<batch-or-slug>/ findings; defines capacity-aware review waves, a coverage ledger, and the per-finding output format.
---

# 10 Fully Review — Global-Codebase Review Workflow

> Per-feature decision rules and closed-finding log live under [04-review/](../04-review/SKILL.md). This file describes the workflow for spawning a global review pass.

## When to use

- Periodic full-codebase audits.
- After a major refactor or before a release that needs broad review coverage.
- Not for single-PR reviews — use [04-review/SKILL.md](../04-review/SKILL.md) for those.

---

## Workflow

```
1. Build a coverage ledger from the current workspace members and the non-Rust
   surfaces in scope. At minimum inspect `Cargo.toml` membership; do not assume a
   root `src/` exists.
2. Split the ledger into non-overlapping review units. Schedule units in waves
   that leave one concurrency slot for the main agent; never assume a fixed
   number of child-agent slots.
3. For each unit, spawn a subagent with these pre-loads:
     - skills/04-review/SKILL.md  (owning-function index)
     - skills/04-review/knowledge-routing.md  (classification and decision scope)
     - skills/04-review/security/mandatory-design/iron-rules.md  (timing / password / cache-key rules)
     - skills/03-coding/01-log-safety.md  (data-plane tracing prohibition)
     - Relevant topic dirs from 04-review/ matching the subtree content.
4. Each subagent verifies findings by reading cited file:line + searching call sites;
   drops speculative, stylistic, or closed items before reporting.
5. Each finding is emitted as a standalone file with: ID, Disposition
   (`confirmed` or `confirmed-downgraded`), Severity, Location, Summary,
   Details, Impact, Suggested fix, and Notes. Name it `<area>-NN-<slug>.md`.
6. Collect finding files into `tasks/review-<batch-or-slug>/` and generate `INDEX.md`
   (columns: ID, Severity, Dimension, Title).
   `INDEX.md` is batch metadata, never a finding.
7. Search the owning function pages by symbol, behavior, and ID. Compare current
   evidence with the rule scope and re-evaluation triggers. Drop an already resolved
   finding only after confirming the same behavior still holds; superseded guidance
   cannot suppress a current defect.
8. Before finishing, mark every ledger unit complete, skipped with a reason, or
   uncovered. A global review is not complete while any in-scope unit is merely
   absent from the ledger.
```

### Coverage-ledger example

Typical units include all seven workspace members (`edgion-dsl`,
`edgion-resources`, `edgion-common`, `edgion-gateway`, `edgion-controller`,
`edgion-cli`, and `edgion-tests`). Split large members such as
`edgion-gateway` by module, but keep a parent ledger row so `backends`, `tls`,
`link_sys`, `conf_sync`, `observe`, `config`, `services`, and other siblings are
not silently omitted. Add manifests, build scripts, CI, deployment, CRDs, and
docs when they are part of the requested review scope.

---

## Dimensions and cross-references

| Dimension | Where to load |
|-----------|---------------|
| Security | [04-review/security/mandatory-design/iron-rules.md](../04-review/security/mandatory-design/iron-rules.md), plus topic dirs: [auth-timing/](../04-review/auth-timing/), [hmac-auth/](../04-review/hmac-auth/), [oidc/](../04-review/oidc/), [ssrf/](../04-review/ssrf/), [ip-restriction/](../04-review/ip-restriction/) |
| Correctness | [04-review/concurrency/](../04-review/concurrency/), [04-review/concurrency/](../04-review/concurrency/), [04-review/handler-requeue/](../04-review/handler-requeue/) |
| Observability | [04-review/observability/](../04-review/observability/), [03-coding/01-log-safety.md](../03-coding/01-log-safety.md) |
| Architecture | [04-review/review-method/](../04-review/review-method/) |
| CPU / memory | [04-review/cpu-memory/](../04-review/cpu-memory/), [04-review/cpu-memory/](../04-review/cpu-memory/) |

---

## Review knowledge

Function indices under `skills/04-review/` route to semantic decision pages classified
by knowledge type. Read `skills/04-review/knowledge-routing.md` for their authority and
maintenance rules. A global review produces open findings under `tasks/`; it does not
close them or declare new accepted tradeoffs. Resolution workflows rewrite the owning
knowledge page after verification, preserving source IDs and evidence there.
