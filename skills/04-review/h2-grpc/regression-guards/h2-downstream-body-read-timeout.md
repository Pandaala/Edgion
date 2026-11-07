---
name: h2-downstream-body-read-timeout
description: Use when reviewing downstream request-body stall / slow-upload DoS findings; covers the fork-side H2 per-chunk read timeout that closed the "stalled upload pins a proxy pump" gap.
status: fixed
finding_id: transport-gap-07
closed: 2026-08-04
knowledge_type: regression-guards
---

# H2 downstream request-body reads are bounded by `read_timeout` — FIXED

## Conclusion

The Edgion fork applies a per-chunk read
timeout to HTTP/2 downstream request-body reads, closing the pre-existing gap
where a client stalling mid-upload could pin a proxy pump (plus its downstream
and upstream connections) indefinitely. HTTP/1 always had this bound
(`v1/server.rs` `read_body`, default 60s); the H2 session lacked it and the
unified `HttpSession::set_read_timeout` H2 arm was a documented no-op that
silently swallowed the timeout Edgion already sets on every request
(`pg_early_request_filter.rs`).

The original fork evidence below is historical. Revalidate the currently locked
fork and Edgion consumer before deciding whether a new stalled-upload finding
is a regression; the old resolution alone cannot dismiss it.

## Core Rationale

**1. The root cause was fork-side asymmetry, not a missing Edgion knob**

Edgion sets `session.set_read_timeout(Some(client.read_timeout))` (from
`EdgionGatewayConfig.spec.httpTimeout.client.readTimeout`, default 60s) on
every request. The fork honored it for H1 and dropped it for H2. The fix adds a
`read_timeout` field to the H2 server session, wraps the
`request_body_reader.data().await` in `read_body_bytes` with it, and routes the
`HttpSession::set_read_timeout` / `get_read_timeout` H2 arms. No Edgion code
change was needed.

**2. Inter-chunk semantics, identical to H1**

The bound is rearmed by every DATA payload carrying at least one byte (and by
END_STREAM): a stalled upload is released after one `read_timeout` of silence,
while a slow-but-progressing upload is never limited in overall duration. Both
directions are pinned by fork tests
(`test_read_body_timeout_releases_stalled_upload`,
`test_read_body_timeout_spares_progressing_upload` in
`pingora-core/src/protocols/http/v2/server.rs`).

Three properties are load-bearing and each has its own regression test in the
same file:

- **It is a DEADLINE carried on the session, not a duration rebuilt per call.**
  Every caller polls the body read from a `tokio::select!` branch, so any other
  ready branch (an upstream response chunk, a cache task, a custom message)
  cancels the read future. A per-call duration would restart from zero on the
  next loop iteration, so a client stalling against an upstream that speaks
  once per period (SSE, gRPC server-streaming, a slow multi-chunk 200) would
  never be released — the DoS would still be open.
  `test_read_body_timeout_survives_select_cancellation`; H1 carries the same
  deadline and the same test
  (`test_read_http_body_timeout_survives_select_cancellation`).
- **An empty DATA frame is not progress.** A zero-length DATA without
  END_STREAM is legal, costs 9 bytes, consumes no flow-control window and
  trips no h2 flood counter, so rearming on it would be a free unlimited
  rearm — and `body_read` never advances, so no byte-count body-size limit
  would catch it either. `test_empty_data_frames_do_not_rearm_read_timeout`.
- **`Content-Length: 0` without END_STREAM is finished, not failed.** For that
  shape the transport has already promised there are no body bytes, so the
  expiry answers `Ok(None)` (finishing the read side) instead of erroring;
  otherwise a silent upstream would turn an about-to-succeed exchange into a
  4xx. `test_read_body_timeout_finishes_declared_empty_body`.

**2b. Default-on, and CONNECT is exempt**

The H2 session now defaults to `Some(60s)`, matching the H1 session's own
default. This is what makes the closure real: nothing in `pingora-core` or
`pingora-proxy` calls `set_read_timeout`, so an opt-in bound would leave every
consumer that does not set one — including pingora's own server apps — with an
unbounded H2 body read while the same application over H1 was protected.
Edgion's per-request `set_read_timeout` overrides it as before.

CONNECT (plain and RFC 8441 extended, both `:method = CONNECT`) is exempt
regardless of what is set: for a tunnel the request body IS the uplink and a
long idle period is ordinary (an idle SSH session, a WebSocket whose traffic
runs server-to-client only). `test_read_body_timeout_spares_connect_tunnel`.
H1 upgrades are NOT exempt — they have carried H1's 60s default since long
before this work, and removing an existing bound is a different (and
DoS-relevant) decision from not adding a new one.

Downstream `ReadTimedout` maps to 408, not 499 or 400
(`pg_fail_to_proxy.rs`): the client stalled its request-body upload but did not
necessarily close the connection, and the request was never malformed. Edgion
records `BodyReadTimeout`, does not record `ClientClosed`, and does not attach
the client-side timeout to the selected backend's attempt error or local-origin
outlier counter. The downstream session is non-reusable. HTTP receives the
terminal 408 response; gRPC and GRPCRoute requests use the canonical error
dispatch and HTTP-to-gRPC status mapping.

**3. Uniform application to gRPC (deliberate product decision, 2026-08-04)**

The timeout applies to all H2 requests including gRPC. A client-streaming/bidi
gRPC request whose client idles longer than `readTimeout` between messages is
now cut, matching the direction of Envoy's `stream_idle_timeout` (default 5
min, also applies to gRPC). A content-type exemption was considered and
rejected because an attacker could bypass the bound by labeling the request
`application/grpc`. Operators with legitimately idle client streams raise
`readTimeout`; documented in
`docs/en/ops-guide/configuration/edgion-gateway-config.md`.

## Fix Suggestions Not Accepted

- Exempt `application/grpc` content-types from the timeout — reopens the DoS
  for any attacker who sets that header; the uniform bound was the decided
  policy.
- A total-upload-duration timeout — wrong semantics; it would kill large slow
  legitimate uploads. The inter-chunk idle bound is the H1-parity contract.
- Bounding the timeout by a plain per-call `timeout(read_timeout, ..)` — see
  the deadline bullet above; it is rearmed by unrelated select branches and
  leaves the DoS open.
- Exempting CONNECT by content-type/upgrade sniffing instead of by method —
  `:method = CONNECT` is the transport fact and covers extended CONNECT too.
- Bounding `trailers().await` as well — `data()` returning `None` requires the
  end of the data section to have been observed, so the trailer poll does not
  wait on the wire.

## Re-evaluation Triggers

Re-open this decision only if:

- Per-route or per-protocol timeout configuration is added (then the uniform
  vs. gRPC-exempt tradeoff can be revisited with real granularity).
- The fork's H2 body-read path is restructured so `read_body_bytes` is no
  longer the single client-facing await point (note `poll_read_body_bytes` is
  deliberately NOT bounded and has no in-tree consumer; bounding it is a
  prerequisite for gaining one).
- H2 tunnels gain a knob of their own, or H1 upgrades are revisited — today
  tunnels are exempt by method and H1 upgrades keep H1's default.

## Reference Cases

- `tasks/todo/transport-coverage-observability-gaps.md` gap 7 (origin of the
  finding; recorded as deliberately untested until the product decision).
- `tasks/todo/important_test/04-fix-the-product-before-writing-the-test.md`
  Item A (the decide-then-fix-then-test rule this closure followed).
- Fork commit on `Pandaala/pingora` branch `edgion`:
  `pingora-core/src/protocols/http/v2/server.rs` +
  `pingora-core/src/protocols/http/server.rs`.
- Edgion consumer: `edgion-gateway/src/routes/http/proxy_http/pg_early_request_filter.rs`.
- Edgion terminal classification:
  `edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs`; backend
  local-origin exclusion:
  `edgion-gateway/src/routes/http/proxy_http/pg_error_while_proxy.rs`.

## Resolution references

- `transport-gap-07` — 2026-08-04.

## Live Gateway timeout coverage

Source task: `add-downstream-request-body-timeout-408-integration`.
The missing coverage was a real test gap, separate from the production 408
classification already implemented in `pg_fail_to_proxy.rs` and the downstream
failure exclusion in `pg_error_while_proxy.rs`.

`edgion-tests/src/client/suites/http_route/transport/downstream_timeout.rs`
adds H1-to-H1 and H2-to-H2 cases to the existing Transport suite. Each keeps an
unfinished 1024-byte request open after sending 48 bytes, proves those bytes
reached the expected raw backend, then waits for the base
`ClientTimeout.read_timeout` of 60 seconds. The dedicated route sets
`backendRequest: 120s` so the upstream timeout cannot win. Each case has a
100-second outer deadline and bounded response/log/metric waits; H2 connection
drivers abort on drop, including failed assertions and cancellation.

Required observations are wire and access-log status 408, `BodyReadTimeout`
present, `ClientClosed` absent, exactly one backend attempt without errors,
and no completed upload. Dedicated Service identities have an outlier policy
with `consecutiveLocalOriginFailures: 1`; the local-origin ejection counter must
not advance. A subsequent real backend reset must increment that same counter
exactly once, proving the policy and metric selection are live rather than
letting a missing policy pass the negative assertion.
The positive control ends with a bounded healthy-echo recovery probe, so an
immediate rerun does not inherit its temporary endpoint ejection.

Historical run on 2026-09-09 against the then-current `edgion_v4` lockfile
commit `518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e` (not repeated at the current
`edgion_v0.9.1@e5f4287` lock):

- `run_integration.sh --no-prepare -r HTTPRoute -i Transport` passed on the
  final build: 74 reported cases, zero failures or infrastructure errors
  (the existing matrix retains its explicitly skipped unsupported combinations).
  Both new live timeout cases passed in approximately 61 seconds, including
  the reset positive control and recovery barrier. The run's final metrics,
  unknown-field, requeue-loop, and cache-residue checks also passed.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed; Clippy retained existing
  unrelated warnings. Product/test binaries built successfully.
- Agent-doc validation, SSA force, metrics inventory, and Pingora source policy
  guards passed. A fresh independent reviewer approved the final test logic,
  resource fixtures, helper lifecycle, and knowledge classification.

Disposition: the integration test gap is fixed; no production behavior changed.
Coverage does not establish cross-protocol combinations, slow-but-progressing
uploads, CONNECT exemptions, gRPC mapping, or request-body plugin capture.
The historical fork tests above remain separate evidence for those boundaries.
Re-evaluate these cases when the shared client timeout, route deadline, metric
labels, outlier threshold semantics, or transport helper lifecycle changes.
