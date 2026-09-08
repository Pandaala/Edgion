# B0178 — HTTP Connected-to-Upstream Callback

## Scope

- Candidate analysis HEAD: `0cd64e0debc9d43634d9035a9382ff8ede92d300`; freshness-revalidated and promoted unchanged at `879bdbe03ca34c84534fe281b85aa506a07c4bd1`
- Pipeline state: terminal; examined-no-change
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/http/proxy_http/pg_connected_to_upstream.rs`
- Technical implementation allow-list: none; examined-no-change candidate

## Handoff

- Analysis: `b0170_final_review` read the complete callback and ProxyHttp dispatch, pinned Pingora invocation paths, attempt/LB/retry ownership, BoringSSL SAN validation and pool isolation, access-log delivery, protocol lifecycles, tests, frozen integrations, and review precedent.
- Decision: examined-no-change. The small hot callback correctly fails SAN validation before accounting, records one connect timestamp on the active attempt, commits exactly one cancellation-safe LB guard, and emits the required immediate canonical access-log entry only for long-lived WebSocket/gRPC-Web traffic.
- Preserved behavior: SAN fail-close, attempt ownership, LeastConn/EWMA cleanup, response-header latency sampling, streaming protocol logging, retries, and observability remain unchanged.
- Testing: LB attempt 7/7, proxy logging 9/9, and backend SAN 10/10 passed. Existing integrations remain frozen; two independent documentation/coverage tasks capture the only gaps.
- Review: a fresh independent reviewer approved examined-no-change with no Must Fix or Should Fix finding. The reviewer independently confirmed SAN ordering and redaction, attempt-guard ownership across retry/cancellation/terminal cleanup, response-header-owned EWMA sampling, and canonical immediate access-log ownership.
- Verification: `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed.
- Follow-ups:
  - `tasks/todo/refresh-connected-upstream-lb-lifecycle-documentation.md`
  - `tasks/todo/add-streaming-conn-est-access-log-integration.md`
- Commit: pending; never push.
