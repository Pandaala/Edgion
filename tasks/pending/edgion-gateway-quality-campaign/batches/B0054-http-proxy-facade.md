# B0054 — HTTP Proxy Facade

## Scope

- Baseline: `7edf2384b59f80736182717394f24561d8bd683c`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- File: `edgion-gateway/src/routes/http/proxy_http/mod.rs`.
- Allowed code or knowledge files: none; analysis concluded `examined-no-change`.
- Existing `edgion-tests/**` files, unrelated modules, and protected user paths remain forbidden.
- Exact commit allow-list: B0053/B0054 campaign ledger, batch, main, and changelog records only.

## Context and value gate

- Current-HEAD analysis verdict: `examined-no-change` after B0053 freshness refresh.
- The facade maps Pingora's request, upstream, response, body, error, and terminal callbacks to explicit single-purpose modules without duplicating policy.
- Listener-lifetime state is cached, request state is snapshot-based, request-body capture uses the canonical bounded storage, and the main upstream response remains streaming with typed terminal delivery.
- The universal `logging()` callback retains canonical access-log and metrics ownership; DirectPeerIp/RemoteIp and response termination semantics remain separated correctly.
- Visibility tightening, a proxy constructor, callback forwarding macros, legacy-hook removal, or an unprofiled async body gate would add churn without a demonstrated correctness or performance benefit.
- B0053 changed no proxy runtime file and clarified that HTTPRoute publishes route/plugin inputs while typed EBTP owns backend/LB policy separately; this is consistent with the facade analysis.

## Implementation

- None. No material improvement cleared the value gate.

## Verification plan

- Independent no-change review against the current Pingora fork and full callback module.
- Full campaign repository gates and frozen-path verification.

## Independent review

- An independent reviewer approved the current-HEAD `examined-no-change` verdict with no Must Fix or deferred finding.
- The review reconciled all Pingora callbacks, request/config/plugin snapshot ownership, response-head commitment, typed body/trailer terminal delivery, error rendering, identity separation, and universal logging.
- The rejected visibility, builder, macro, legacy-hook, warning, and unprofiled body-gate alternatives still lack concrete value.

## Verification

- The full `routes::http::proxy_http` library suite passed all 285 tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the campaign's known Kafka and frozen integration-test clippy warnings plus cargo-deny duplicate/advisory-ignore warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `de8abb130be440c7c104dd0934c547d2a3241b03`.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
