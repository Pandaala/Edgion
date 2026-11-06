---
name: h2-writer-capacity-stall-after-response
description: Historical review evidence for Pingora fork / H2 request-body pump; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 writer capacity stall after response

## Recorded conclusion

An upstream that flags END_STREAM and then withholds request-body flow-control window without resetting must not block the write forever: the pump awaits that write inline, so the duplex loop stops delivering the response and the client is answered never. Wire END_STREAM plus a stalled write authorizes abandoning the upload and DELIVERING the response; failing the exchange, bailing on END_STREAM alone with no timer, adding a `Notify` to `StreamRecord`, cancelling `write_body` on a probe tick, and making the probe interval a peer option are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-writer-capacity-stall-after-response.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-writer-capacity-stall-after-response.md)

## Resolution references

- `H2-007` — 2026-08-27 (fixed in pingora fork `edgion_v3`).

## Gateway regression boundary

`edgion-h2-upstream-lifecycle-e2e-validation` adds product verification in
`edgion-tests/src/client/suites/http_route/transport/h2_lifecycle.rs`. Its
`stall` control consumes exactly the origin's default 65,535-byte upload
window without returning credit, while the client submits a 1 MiB body. The
recorder must show an open upload, a complete 64-byte response, then Gateway
`CANCEL`, all within an 18-second bound. A second stalled upload must complete
on the same upstream connection without an additional connection accept.

The `/lifecycle` HTTPRoute explicitly disables `backendRequest`, so the fork's
10-second EOS stall probe (`UPSTREAM_STALL_PROBE_INTERVAL`), rather than a
route write timeout, supplies the bound. In the same unconfigured case the
write also carries the fork's 10-second `DEFAULT_H2_UPSTREAM_WRITE_TIMEOUT`
floor; once `upstream_response_ended` is observed, either the probe
(`UpstreamBodyWriteEnd::StalledAfterResponse`) or that floor's `WriteTimedout`
(`upstream_write_error_outcome` via `upstream_write_stalled_after_response`)
abandons the upload as `UpstreamDoneReceiving` and the response is delivered. Backend wire proofs check the handshake window, absence of stream
WINDOW_UPDATE before response completion, full response EOS, and upload reset.
Ordinary early-response coverage does not establish this trigger. Same-socket
reuse establishes capacity recovery, not the absence of every pump-task leak.

Current source inspection uses the Cargo.lock resolution
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`, `h2 0.4.20`. It was
re-verified by source inspection at that revision on 2026-11-06 after the fork
was rebased onto upstream 0.9.1; `pingora-proxy/src/proxy_h2_request_body.rs`
is unchanged from the earlier `edgion_0.9.0_v1@0b0d8bba` lock.
Historical run (against `0b0d8bba`, not repeated for `e5f4287`): on
2026-10-06, `cargo test -p edgion-tests --bin test_server transport::h2
-- --nocapture` passed 14 tests, including all five lifecycle wire proofs.
The final fresh-Gateway Transport run passed all 165 registered entries,
including all four H1-to-H2/H2-to-H2 stall and stall-reuse scenarios.
See [Gateway reuse verification scope](h2-proxy-cache-reuse-harness-consumers.md#gateway-reuse-verification-scope)
for the exact production/fork revisions, launch commands, executed/skip counts,
and outer-wrapper environment limitation. Interrupted runs are not passing
results; private fork tests were not freshly run.
