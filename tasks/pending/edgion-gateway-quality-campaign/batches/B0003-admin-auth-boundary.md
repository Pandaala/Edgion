# B0003 — Admin Authentication Boundary

## Scope

- Baseline: `48643849b5102abb4a4c54524aeb7bd66b4d9bdd`
- Pipeline state: `terminal`
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
- Freshness: B0002 changed only `build.rs` and campaign records; B0003 targets, call chains, tests, and knowledge inputs are unchanged from analysis.
- Deferred observations: a frozen integration comment has a stale source path (D0003), and the config-client redaction review page has a stale removed design link (D0004).

## Implementation

- Implementation subagent: `b0002_analysis` in a distinct B0003 implementation turn.
- Complexity: simple.
- Actual change: added exact `WWW-Authenticate: Bearer realm="edgion-gateway-admin"` on configured-token 401 responses, middleware-level regression tests, and current review-knowledge corrections; `api/mod.rs` required no edit.
- Preserved behavior: 503 plus `Retry-After: 10` without challenge, generic bodies, digest-only constant-time token checks, merged-router protection, probe separation, config precedence, redaction, and observability surfaces.
- Focused verification: 9 Admin auth tests, formatting, agent-doc validation, English-only, diff check, and frozen integration paths passed.
- Review/adjudication: initial review rejected scheme-only `Bearer`; independent adjudication applied RFC 6750 §3 and requires `Bearer realm="edgion-gateway-admin"` for every configured-token 401 plus stable symbol-based citations in the edited testing-router knowledge page.

## Independent review

- Reviewer: `b0001_review`; adjudicated correction re-reviewed by `coverage_plan`, both independent of the B0003 analyst and implementer.
- Verdict: LGTM after RFC 6750 correction; no remaining Must Fix or Should Fix.
- Confirmed behavior: identical generic 401 bodies/challenges for missing, malformed, and wrong credentials; correct-token handler reachability; unchanged 503/probe/redaction/observability boundaries.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Baseline unchanged before delegation. |
| Targeted unit tests | passed | `cargo test -p edgion-gateway --lib api::admin_auth`: 9 passed. |
| Formatting and workspace compile | passed | `cargo fmt --all`; `cargo check --workspace --all-targets`. |
| Workspace lint | passed | Exit 0 with pre-existing warnings outside this batch. |
| Repository guards | passed | Agent docs, SSA, metrics, Pingora source, Gateway no-tracing, English-only, and full `cargo deny check`. |
| Frozen integration paths after verification | passed | All 3,442 existing entries remain content/mode clean. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
