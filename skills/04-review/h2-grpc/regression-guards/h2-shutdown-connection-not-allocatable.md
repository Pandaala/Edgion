---
name: h2-shutdown-connection-not-allocatable
description: Historical review evidence for Pingora fork / H2 connection pool; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 shutdown connection not allocatable

## Recorded conclusion

`mark_shutdown()` is enforced where a stream is allocated (`ConnectionRef::spawn_stream`, checked before `new_stream()`, plus a defensive post-await re-check that cannot fire while `Stub::new_stream` clones `SendRequest`) and where the pool picks a candidate (`Connector::reused_http_session`), not only at re-pooling. "`more_streams_allowed()` already checks the flag" is not accepted: it runs after the stream is handed out. Nothing evicts a marked connection from the in-use pool until its last stream is released, so a read-timeout mark used to amplify one hung upstream stream into a run of them. The guarantee is at the allocation boundary, not the whole request: the `spawn_stream` -> `send_request` gap remains open by design, matching GOAWAY's "committed work finishes" semantics. Filtering at selection only, guarding inside `spawn_stream` only, and returning an error instead of `Ok(None)` are also not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-shutdown-connection-not-allocatable.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-shutdown-connection-not-allocatable.md)

## Resolution references

- `H2-008` — 2026-08-27 (fixed in pingora fork `edgion_v3`).

## Gateway regression boundary

`edgion-h2-upstream-lifecycle-e2e-validation` adds a real-Gateway `shutdown`
scenario in `edgion-tests/src/client/suites/http_route/transport/h2_lifecycle.rs`.
The upstream request hangs behind the existing two-second `/timeout` rule.
Its downstream 504, upstream CANCEL, and successful follow-up on exactly one
new connection establish the product's timeout and retirement outcome.
The Gateway retains `PeerOptions::default().max_h2_streams = 1`; it cannot
establish a concurrent upstream sibling contract.

The locked fork (`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`,
`h2 0.4.20`) marks the connection in the upstream read-timeout failure arm
(`pingora-proxy/src/proxy_h2.rs`, `DuplexPumpOutcome::Failed`), then checks
shutdown at both connector selection (`Connector::reused_http_session`, in-use
and idle filters) and stream allocation (`ConnectionRef::spawn_stream`, before
`new_stream()` plus the post-await re-check) in
`pingora-core/src/connectors/http/v2.rs`. This was re-verified by source
inspection at that revision on 2026-11-06 after the fork was rebased onto
upstream 0.9.1; `connectors/http/v2.rs` is unchanged from the earlier
`edgion_0.9.0_v1@0b0d8bba` lock. The supplemental
`h2_lifecycle_connector.rs` uses this exact dependency's public Connector
with two slots. After an actual ReadTimedout, it explicitly calls
`mark_shutdown()`, drops only the fault session, and checks that a live,
non-idle marked connection returns `Ok(None)` from `spawn_stream()`.
Connector selection must also open a new connection while the accepted
sibling remains pending; a recorded release request then permits that sibling
to return all 64 bytes. The explicit mark is an allocation/pool contract probe,
not evidence that the Gateway automatically marked that particular connection.
Together with the serial Gateway case and source inspection it covers the
reported boundaries without enabling product multiplexing. This does not
close the separate failure-case disposition policy task or prove all races.

Historical run (against the earlier `edgion_0.9.0_v1@0b0d8bba` lock, not
repeated for `e5f4287`): on 2026-10-06, `cargo test -p edgion-tests --bin test_server transport::h2
-- --nocapture` passed 14 tests, including the held-stream wire proof.
The final fresh-Gateway Transport run passed all 165 registered entries,
including both serial Gateway shutdown scenarios and the supplemental public
Connector shutdown-allocation scenario. See
[Gateway reuse verification scope](h2-proxy-cache-reuse-harness-consumers.md#gateway-reuse-verification-scope)
for exact production/fork revisions, launch commands, executed/skip counts,
and the outer-wrapper environment limitation. No private fork allocation-race
tests were freshly run; interrupted runs are not passing results.
