# B0180 — HTTP Error While Proxying

## Scope

- Candidate analysis and freshness-revalidation HEAD: `a7cb2350b4be7a5d5a6ff25ea8337c7f607eb6f2`
- Target blob: `67cbaeba804d3b620f755018e4fd81aa8def15da`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned code: `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`
- Frozen support allow-list:
  - `skills/04-review/routing/regression-guards/1xx-response-filter-state-pollution.md`
  - `skills/04-review/routing/regression-guards/per-try-timeout-test-coverage.md`
  - `skills/04-review/routing/intentional-design/per-try-timeout-via-backend-request.md`
  - `docs/en/user-guide/http-route/resilience/retry.md`
  - `docs/zh-CN/user-guide/http-route/resilience/retry.md`

## Handoff

- Analysis: `b0169_final_review` read the complete callback, ProxyHttp/Pingora error and retry loops, H1 response writer, response filtering, relay state, LB/outlier/AI ownership, tests, frozen integrations, and review precedent.
- Finding: Case 1.5 tests raw `response_written().is_none()`, but Pingora records non-terminal 100/102/103 heads there. A 100 followed by a backend-request timeout therefore suppresses an otherwise configured retry, contrary to Pingora's final-response contract and Edgion's existing `has_final_response_written` helper.
- Intended fix: reuse the canonical helper only at the Case 1.5 gate, add discriminating H1 session tests for 100 versus 101/200, and clarify only directly affected review/user documentation. Preserve retry counts, budgets, buffering, AI behavior, method safety, error rendering, logging, metrics, and all other decision-tree ordering.
- Integration: `HTTPRoute/Backend/InformationalRetry` now drives the real HTTP/1 sequence through the Gateway: trace attempt 1 flushes `100 Continue` and stalls past `backendRequest`, while attempt 2 returns the bounded 200 echo response. The raw downstream client, upstream recorder, and canonical access log jointly pin the ordered retry and final-response boundary.
- Implementation: Case 1.5 now calls one private predicate containing its exact timeout/source/config/final-response gates. Real H1 sessions prove raw response state is populated for 100/102/103 while the production predicate remains retryable; 101/200 and every other excluded input remain non-retryable. The three existing review pages and bilingual retry guide now use the same final-response boundary without claiming end-to-end coverage.
- Review: the approved plan had no correction. Fresh final review initially rejected helper-only tests because reverting the production call would still pass; the implementer bound the live-session tests to the production predicate, and correction re-review returned LGTM with no remaining Must Fix or Should Fix finding.
- Verification: target tests passed 6/6 and the complete proxy-http suite passed 230/230. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed.
- Commit: `f8413d67`; never pushed by the campaign.
