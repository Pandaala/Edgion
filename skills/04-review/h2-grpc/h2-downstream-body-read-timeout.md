---
name: h2-downstream-body-read-timeout
description: Use when reviewing downstream request-body stall / slow-upload DoS findings; covers the fork-side H2 per-chunk read timeout that closed the "stalled upload pins a proxy pump" gap.
status: fixed
finding_id: transport-gap-07
closed: 2026-08-04
---

# H2 downstream request-body reads are bounded by `read_timeout` — FIXED

## Conclusion

The fork (`Pandaala/pingora`, branch `edgion`) now applies a per-chunk read
timeout to HTTP/2 downstream request-body reads, closing the pre-existing gap
where a client stalling mid-upload could pin a proxy pump (plus its downstream
and upstream connections) indefinitely. HTTP/1 always had this bound
(`v1/server.rs` `read_body`, default 60s); the H2 session lacked it and the
unified `HttpSession::set_read_timeout` H2 arm was a documented no-op that
silently swallowed the timeout Edgion already sets on every request
(`pg_early_request_filter.rs`).

Re-reports of the form "H2 request body read is unbounded / a stalled upload
pins the pump / `set_read_timeout` is a no-op for h2" are **not accepted**.

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

Downstream `ReadTimedout` now maps to 408, not 400 (`proxy_trait.rs`
`fail_to_proxy`): the client stopped sending, the request was never malformed.

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
