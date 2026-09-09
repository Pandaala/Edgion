# B0182 — HTTP Fail to Proxy

## Scope

- Candidate analysis HEAD: `a7cb2350b4be7a5d5a6ff25ea8337c7f607eb6f2`; call-chain freshness-refreshed at `efeb18f641e56f20fcb53ffed8f51bdc2b7a51e3`
- Target blob: `75ac2c5d045ca878ae3c87dd2465f22ad47ea772`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned code: `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`
- Frozen support allow-list:
  - `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`
  - `skills/04-review/h2-grpc/regression-guards/h2-downstream-body-read-timeout.md`
  - `skills/03-coding/observability/00-access-log.md`
  - `skills/04-review/backends/regression-guards/circuit-breaker-local-origin-split.md`

## Handoff

- Analysis: `b0169_final_review` read the full terminal/error callbacks, current B0180/B0181 call chains, pinned Pingora timeout mapping, response ownership, access-log/outlier behavior, tests, frozen integrations, and review precedent.
- Finding: downstream `ReadTimedout` means a stalled client request-body upload and must produce 408, but the override merges it with client disconnect/write failures as 499. That skips the response, records `ClientClosed`, can attach the client failure to the backend attempt, and the earlier error callback can also count it as a backend local-origin failure.
- Intended fix: map only downstream read timeout to 408, keep true downstream disconnect/write failures at 499 and upstream timeouts at 504, preserve literal HTTP statuses, write the canonical HTTP/gRPC terminal response, avoid `ClientClosed` and backend error/outlier attribution for the downstream stall, and leave all retry/AI/budget/body behavior unchanged.
- Integration: existing files were frozen during this batch, so stalled H1/H2 upload wire coverage was deferred. The separately authorized follow-up, `add-downstream-request-body-timeout-408-integration`, is maintained in [the timeout regression guard](../../../../skills/04-review/h2-grpc/regression-guards/h2-downstream-body-read-timeout.md#live-gateway-timeout-coverage).
- Implementation: production predicates now classify only downstream `ReadTimedout` as 408, choose `BodyReadTimeout`, suppress attempt error attribution, and keep the downstream connection non-reusable. True downstream transport failures retain 499/`ClientClosed`; upstream timeouts retain 504; literal statuses remain exact. The preceding error callback now excludes every downstream-sourced failure from backend local-origin outlier recording while preserving all existing status and source behavior.
- Review: the refreshed plan was independently approved with the explicit boundary that the H1-backed GRPCRoute unit proves canonical dispatch/header/status mapping rather than H2 wire framing. A fresh final reviewer confirmed the complete callback composition, exact scope, deferred integration gap, and no Must Fix or Should Fix finding.
- Verification: fail-to-proxy and error-while-proxy tests each passed 6/6, and the complete proxy-http suite passed 237/237. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed.
- Commit: `d2da7f4d`; never pushed by the campaign.
