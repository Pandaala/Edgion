# B0003 — Admin Authentication Boundary

## Scope

- Baseline: `8a32f09f074e3280dd9b500810481c62a72613b6`
- Pipeline state: `analysis-ready`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/api/admin_auth.rs`, `edgion-gateway/src/api/mod.rs`
- Grouping reason: middleware outcome and router protection form one authentication boundary.
- Allowed support files: `skills/04-review/admin-api/SKILL.md`, new `skills/04-review/admin-api/regression-guards/gateway-admin-bearer-boundary.md`, `skills/04-review/testing/intentional-design/testing-router-error-body-enumeration.md`, and `skills/04-review/admin-api/regression-guards/gateway-admin-loopback-default.md`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the two owned files, four approved review-knowledge paths, campaign records, and any standalone deferred-task records created at promotion.

## Context and value gate

- Analysis subagent: `b0001_adjudicate` in a distinct B0003 analysis turn.
- Coverage summary: full Gateway Admin API auth/router boundary, startup token resolution, probes, Controller semantic comparison, frozen integration coverage, history, and review precedent.
- Concrete opportunity: configured-token 401 responses omit the RFC-required `WWW-Authenticate` challenge; current token comparison, merged-router protection, 503 fail-close, probe separation, and redaction are otherwise correct.
- Decision: `change` (simple). Add static `WWW-Authenticate: Bearer` only to the 401 path, focused middleware tests, and current review-knowledge corrections.
- Deferred observations: a frozen integration comment has a stale source path, and another review page has a stale historical design link; evaluate as separate low-value tasks at promotion.

## Implementation

- Implementation subagent: pending.
- Complexity: simple.

## Independent review

- Reviewer: pending.
- Verdict: pending.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline unchanged before delegation. |

## Handoff

- Ledger/changelog updated: no.
- Commit: pending automatic commit.
