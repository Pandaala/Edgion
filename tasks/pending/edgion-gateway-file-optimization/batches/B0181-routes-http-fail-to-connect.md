# B0181 — HTTP Fail to Connect

## Scope

- Candidate analysis HEAD: `a7cb2350b4be7a5d5a6ff25ea8337c7f607eb6f2`; freshness-revalidated and promoted unchanged at `f8413d67450440704b2377e775d330fb047d1273`
- Target blob: `12e1292811f485ab1c74d15841634a81bee52fd5`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned code: `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_connect.rs`
- Frozen support allow-list:
  - `skills/04-review/routing/SKILL.md`
  - `skills/04-review/routing/regression-guards/connect-failure-after-deadline-uses-timeout-outcome.md`
  - `skills/04-review/h2-grpc/regression-guards/timeout-honored.md`

## Handoff

- Analysis: `b0170_final_review` read the full callback, ProxyHttp/Pingora connect-failure and terminal error paths, retry/budget/relay/LB/outlier/AI ownership, deadline mapping, tests, frozen integrations, and review precedent.
- Finding: when an end-to-end request or client gRPC deadline expires during connection establishment, the branch stamps attempt status 504 and suppresses retry but returns the original connect error. Terminal classification can therefore render 502 or the wrong gRPC status and omit the canonical deadline diagnostic.
- Intended fix: select request-before-gRPC typed expiry, record its canonical diagnostic, and return a non-retryable 504 while retaining the original connect error as the binary-owned cause. Preserve attempt/outlier stamping, relay and retry ordering, AI behavior, budgets, logging ownership, and all ordinary connect failures.
- Deferred scope: connection establishment itself is not yet clamped by the remaining end-to-end deadline; track that wider work in `tasks/todo/bound-http-upstream-connect-by-end-to-end-deadlines.md`.
- Implementation: a typed selector makes request expiry win over gRPC expiry. The production deadline outcome records the selected canonical diagnostic, keeps the existing attempt/outlier facts while setting its status to 504, and wraps the original upstream error as the cause of an explicitly non-retryable `HTTPStatus(504)`. Selector and outcome tests cover neither/request/gRPC/both, diagnostic ownership, adversarial inherited retry state, and retained cause type/source.
- Review: the plan was independently approved with discriminating retry/cause test requirements. A fresh final reviewer confirmed terminal HTTP 504 and gRPC status 4 mapping, cause-chain logging, branch ordering, exact scope, and no remaining Must Fix or Should Fix finding.
- Verification: target tests passed 5/5 and the complete proxy-http suite passed 233/233. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed.
- Commit: pending; never push.
