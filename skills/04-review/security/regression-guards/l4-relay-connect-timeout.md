---
name: l4-relay-connect-timeout
description: L4 upstream TCP connect bounds, explicit opt-out, current CRD coverage and LocalOrigin attribution, with historical listener-06 evidence and verification limits.
status: fixed
finding_id: listener-06
closed: 2026-06-14
knowledge_type: regression-guards
---

# L4 Relay Upstream Connect Timeout — FIXED

## Conclusion

Plain TCP, TLS-terminated, and TLS-passthrough proxies apply an
operator-configurable upstream TCP connect timeout through
`BasicPeer.options.connection_timeout`. The default is 10 seconds per connect
attempt; explicit zero-valued durations disable the application-level bound.
The current CRD declares the configuration fields, and connect failures use
`FailureClass::LocalOrigin` for outlier detection.

The historical `listener-06` correction established this connect bound. It does
not prove a total TLS setup deadline, a retry-sequence deadline, or the absence
of other waiting-policy defects. Verify the affected operation rather than
using the closed finding as a blanket rejection of a new report.

## Current implementation

### Peer-owned connect bound

Each connect site sets `peer.options.connection_timeout` with the shared
`resolve_tcp_connect_timeout` immediately before calling
`TransportConnector::new_stream`:

- `edgion-gateway/src/routes/tcp/edgion_tcp.rs`
- `edgion-gateway/src/routes/tls/proxy.rs`
- `edgion-gateway/src/routes/tls/passthrough_proxy.rs`

The inspected Pingora revision `518f727` applies `peer.connection_timeout()`
around the ordinary TCP connect future in `pingora-core/src/connectors/l4.rs`.
`TransportConnector::new(None)` therefore does not mean that the peer's connect
is unbounded; changing connector construction is unnecessary for this bound.

All three upstream connections here are plain TCP. Downstream TLS termination
is separate, and passthrough carries the end-to-end TLS handshake over the
established connection. The option does not bound those TLS handshakes,
post-connect PROXY protocol/ClientHello writes, or TLS retry backoff. The shared
[relay idle deadline](l4-relay-idle-timeout.md) starts with the relay; it is a
separate clock. UDP is outside this connect contract.

### Resource and CRD contract

`TcpTimeout` and `resolve_tcp_connect_timeout` in
`edgion-resources/src/resources/edgion_gateway_config.rs` define:

| Input | Effective connect timeout |
|-------|---------------------------|
| No `tcpTimeout` section, or omitted `connectTimeout` | `Some(10s)` |
| Valid nonzero GEP-2257 duration | `Some(parsed duration)` |
| Explicit zero duration such as `"0s"` or `"0ms"` | `None`; no application-level bound |
| Invalid duration reaching the resolver | `Some(10s)`; silent fail-safe fallback |

The committed
`config/crd/edgion-crd/edgion_gateway_config_crd.yaml` declares both
`spec.tcpTimeout.connectTimeout` and `idleTimeout`, with defaults `10s` and `1h`
and GEP-2257 patterns. These fields no longer have the historical pruning gap.
The pattern rejects bare `"0"`; the resolver also treats it as invalid rather
than as opt-out. The CRD's nearby descriptive text still says `"0"/"0s"`
disables the timeout, which conflicts with its enforced pattern. That remaining
prose discrepancy is not evidence that bare `"0"` is accepted, and the CRD is
outside this documentation-only correction.

### Outlier and access-log ownership

All three connect-error branches record `FailureClass::LocalOrigin` through
`record_failure_for_backend_with_token` when an outlier completion token was
captured. The store checks current backend membership and policy identity;
missing or stale ownership does not create a new failure record. Successful
connects use the corresponding token-bound success path.

`FailureClass::Gateway` remains a distinct HTTP-status classification (502–504),
not the current L4 connect-error bucket. The independent counters are defined
in `backends/outlier_detection/state.rs` and exercised in the store tests.

TCP exposes `UpstreamConnectionFailed` in its canonical connection log. TLS
paths record failed attempts through their existing connection diagnostics;
`connect_time` marks successful establishment, not a dedicated timeout signal.
Do not add per-connection tracing or a parallel per-Service metric to document
this behavior.

## Historical evidence and scope

The original correction was closed on 2026-06-14 as `listener-06`. Its useful
regression boundary is the shared default plus assignment at all three connect
sites. Two statements from that record are historical, not current guarantees:

- The CRD once omitted `tcpTimeout` and pruned its child fields. The former
  `crd-struct-sync-gap-edgion-gateway-config` task reference no longer resolves;
  current schema evidence replaces that open-gap claim for these fields.
- The record described connect failures as `FailureClass::Gateway` with
  address-based recording. Current callers use token-bound `LocalOrigin`
  attribution, so the old description must not guide future changes.

A per-route connect-timeout annotation would require a separately scoped
configuration and publication design. The existing gateway-wide knob does not
justify adding that feature during a narrow timeout review. Likewise, metrics
must follow canonical inventory and cardinality rules rather than introduce
an unbounded `service` label.

## Verification and re-evaluation

This refresh inspected source, CRD fields/patterns, and existing test assertions;
it did not rerun runtime tests or establish fresh wire-level timing evidence.
The relevant verification floor is:

- Resource tests `resolve_connect_*`, omitted/explicit-field tests, and
  `connect_default_const_and_serde_default_stay_in_sync` cover default, explicit
  duration, zero opt-out, and invalid-input fallback.
- `edgion-resources/tests/crd_coverage.rs::gateway_config_spec_has_no_uncovered_fields`
  guards field presence in both directions. It does not validate descriptions,
  patterns, or live Kubernetes admission.
- Outlier store tests `local_origin_counter_is_independent_of_gateway`,
  `gateway_failures_do_not_trip_local_origin_threshold`, and
  `success_resets_local_origin_counter` distinguish the failure buckets.

Re-evaluate when a connect caller, peer/connector implementation, timeout
resolver, CRD, or outlier attribution changes. A new L4 path needs independent
confirmation that it sets and honors the bound. Investigate setup writes and
retry-wide deadlines separately; already-bounded TCP connects do not settle
those questions. User configuration remains documented in
`docs/en/ops-guide/configuration/edgion-gateway-config.md` (`spec.tcpTimeout`).
