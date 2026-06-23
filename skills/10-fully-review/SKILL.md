---
name: fully-review
description: Use when running or preparing a global codebase review pass that produces tasks/review-N-<slug>/ findings; defines the 6-subagent batch workflow and the per-finding output format.
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
1. Define scope: split src/ into 6 non-overlapping subtrees (e.g., by crate / module cluster).
2. For each subtree, spawn a subagent with these pre-loads:
     - skills/04-review/SKILL.md  (routing index + closed-findings pointer)
     - skills/04-review/CLOSED-FINDINGS.md  (skip known false positives)
     - skills/04-review/security/iron-rules.md  (timing / password / cache-key rules)
     - skills/03-coding/01-log-safety.md  (data-plane tracing prohibition)
     - Relevant topic dirs from 04-review/ matching the subtree content.
3. Each subagent verifies findings by reading cited file:line + grep-checking call sites;
   drops speculative, stylistic, or closed items before reporting.
4. Each finding is emitted as a standalone file using the template:
     Severity / Location / Summary / Details / Impact / Suggested fix / Notes
5. Collect all finding files into tasks/review-N-<slug>/ and generate INDEX.md
   (columns: ID, Severity, Dimension, Title).
6. Cross-check every finding against already-closed findings (recorded one-file-per-finding
   under `skills/04-review/<topic>/`; use `grep`/`ls` to find them); findings that match a
   closed entry are dropped or annotated "already closed".
```

### Scope-split example (review-2 pattern)

Six subtrees: `edgion-controller/src/`, `edgion-gateway/src/runtime/`,
`edgion-gateway/src/plugins/`, `edgion-gateway/src/routes/`, `edgion-common/src/`,
and `edgion-resources/src/`.

---

## Dimensions and cross-references

| Dimension | Where to load |
|-----------|---------------|
| Security | [04-review/security/iron-rules.md](../04-review/security/iron-rules.md), plus topic dirs: [auth-timing/](../04-review/auth-timing/), [hmac-auth/](../04-review/hmac-auth/), [oidc/](../04-review/oidc/), [ssrf/](../04-review/ssrf/), [ip-restriction/](../04-review/ip-restriction/) |
| Correctness | [04-review/concurrency/](../04-review/concurrency/), [04-review/rwlock-unwrap/](../04-review/rwlock-unwrap/), [04-review/handler-requeue/](../04-review/handler-requeue/) |
| Observability | [04-review/observability/](../04-review/observability/), [03-coding/01-log-safety.md](../03-coding/01-log-safety.md) |
| Architecture | [04-review/architecture/](../04-review/architecture/) |
| CPU / memory | [04-review/cpu-memory/](../04-review/cpu-memory/), [04-review/memory-leak/](../04-review/memory-leak/) |

---

## Closed findings

Closed findings are recorded one-file-per-finding, scattered by topic across the
`skills/04-review/` subdirectories; use `grep`/`ls` to find them. After each global pass,
record newly closed items as a finding file under the relevant topic directory.
