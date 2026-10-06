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
10-second EOS stall probe, rather than a route write timeout, supplies the
bound. Backend wire proofs check the handshake window, absence of stream
WINDOW_UPDATE before response completion, full response EOS, and upload reset.
Ordinary early-response coverage does not establish this trigger. Same-socket
reuse establishes capacity recovery, not the absence of every pump-task leak.

Current source inspection uses the Cargo.lock resolution
`edgion_0.9.0_v1@0b0d8bba9609655bfd7db0fe0add6818bc0ee937`, `h2 0.4.19`.
On 2026-10-06, `cargo test -p edgion-tests --bin test_server transport::h2
-- --nocapture` passed 14 tests, including all five lifecycle wire proofs.
The final fresh-Gateway Transport run passed all 165 registered entries,
including all four H1-to-H2/H2-to-H2 stall and stall-reuse scenarios.
See [Gateway reuse verification scope](h2-proxy-cache-reuse-harness-consumers.md#gateway-reuse-verification-scope)
for the exact production/fork revisions, launch commands, executed/skip counts,
and outer-wrapper environment limitation. Interrupted runs are not passing
results; private fork tests were not freshly run.
