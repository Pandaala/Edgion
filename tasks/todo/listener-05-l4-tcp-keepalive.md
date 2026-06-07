# listener-05: TCP keepalive for L4 (TCP/TLS) upstream connections

**Source review finding**: `tasks/review-envoy/listener-05-tcp-l4-no-keepalive-or-nodelay.md`
(promoted to a task on 2026-05-31 via fix-review-issue Scope gate option A — adding an
operator-facing knob is wider than the narrow review-fix workflow).

**Severity**: low · **Verdict**: ✅ real gap (robustness) · **Value**: 🟡 optional fix

## Problem summary

L4 proxy paths build the upstream peer with `BasicPeer::new(addr)`, whose default
`PeerOptions.tcp_keepalive` is `None`. Pingora only enables SO_KEEPALIVE when
`peer.tcp_keepalive()` returns `Some(..)` (`pingora-core-0.8.0/src/connectors/l4.rs:195`),
so L4 upstream connections send **no** keepalive probes. An idle connection whose NAT /
firewall state has been dropped goes half-open silently; the next write fails only after
the OS TCP retransmit timeout (~15 min on Linux), holding the paired downstream connection
open for the whole duration. There is currently no annotation or CRD field to enable it.

Note: the original finding's TCP_NODELAY claim is **false** — Pingora calls
`set_nodelay()` unconditionally on every L4 connection (`l4.rs:198`). Only keepalive is
missing. `grep` confirms `tcp_keepalive` / `TcpKeepalive` appear nowhere in `src/` today.

The three data-plane consumers (all construct `BasicPeer::new`):
- `src/core/gateway/routes/tcp/edgion_tcp.rs` — TCPRoute (plain TCP)
- `src/core/gateway/routes/tls/proxy.rs` — TLSRoute (TLS terminate → TCP)
- `src/core/gateway/routes/tls/passthrough_proxy.rs` — TLSRoute (TLS passthrough)

## Envoy baseline (reference)

Envoy exposes `upstream_connection_options.tcp_keepalive` at **cluster** level
(≈ per-backend ≈ per-route). All three fields are optional and **default OFF** — when
unset, no keepalive probes are sent (same as Edgion today). The value is in *providing the
opt-in knob*, not in defaulting it on:
- `keepalive_time` — idle seconds before the first probe
- `keepalive_interval` — seconds between probes
- `keepalive_probes` — failed probes before the connection is considered dead

## Design decision (locked in Step 1 of fix-review-issue)

**Placement: Route-level annotations** (single layer), NOT a new `EdgionGatewayConfigSpec`
field. Rationale:
1. Matches Envoy's per-cluster (≈ per-route) granularity.
2. Consistent with the existing L4 per-route upstream knobs, which all live in annotations:
   `edgion.io/proxy-protocol` and `edgion.io/max-connect-retries` are parsed in
   `tls/conf_handler_impl.rs` → written onto `rule` → consumed in `proxy.rs`.
   (Constants in `src/types/constants/annotations.rs`, `edgion` module.)
3. Annotations are `map<string,string>` — no CRD schema / JsonSchema / CRD-yaml change,
   smaller blast radius than a spec field.

**Default OFF**: with no annotation present, behavior is byte-for-byte identical to today
(aligns with Envoy's opt-in semantics).

Proposed annotation keys (mirror Envoy's three fields):
| annotation | Envoy field | unset behavior |
|---|---|---|
| `edgion.io/tcp-keepalive-time` | `keepalive_time` | absent = keepalive disabled |
| `edgion.io/tcp-keepalive-interval` | `keepalive_interval` | OS default |
| `edgion.io/tcp-keepalive-probes` | `keepalive_probes` | OS default |

Enablement trigger: keepalive is configured only when `tcp-keepalive-time` is present and
parses to a positive integer; the other two are refinements applied on top.

## Implementation outline

1. Add the three annotation key constants to `src/types/constants/annotations.rs` (`edgion`).
2. **TLSRoute** (`tls/conf_handler_impl.rs`): parse the keepalive annotations next to the
   existing `proxy_protocol_version` / `max_connect_retries` block; cap to sane bounds; warn
   on out-of-range (mirror the `MAX_TLS_CONNECT_RETRIES` cap pattern). Store the resolved
   `Option<TcpKeepalive>` on the rule (new field, alongside `max_connect_retries`).
3. **TCPRoute** (`tcp/conf_handler_impl.rs`): this path has **no** annotation→rule upstream
   knob today (only `resolve_stream_plugin_store_key` reads `route.metadata`). A new parse
   channel must be added here, and a field carried on the TCP rule type. This is the largest
   net-new piece; TLS side has a reusable pattern to copy.
4. At each `BasicPeer::new` site (3 listed above), set
   `peer.options.tcp_keepalive = rule_keepalive.clone();` before `new_stream(&peer)`.
   Use Pingora's `pingora::protocols::l4::ext::TcpKeepalive` (verify exact path/fields at
   pingora 0.8.0; confirm `count` / `idle` / `interval` field names).

## Design questions to resolve before coding

- **Bounds / caps**: what min/max should `time` / `interval` / `probes` be clamped to?
  (Mirror `cap_tls_connect_retries`.) Reject zero/negative → disabled.
- **TCPRoute scope**: confirm we add the net-new TCP annotation channel now (recommended,
  so L4 coverage is symmetric and matches the finding, which names both paths), vs deferring
  TCP and shipping TLS-only first.
- **Observability**: do we want a per-route log line when keepalive is enabled (mirroring the
  existing "TLSRoute configured with extended options" info log)? No data-plane `tracing::`
  on the hot path (Iron rule 3) — this is conf-handler time only, so it is allowed there.
- **Docs**: add the three annotations to `docs/en` (and `docs/zh`) annotation reference and
  `skills/02-features/05-annotations/`.
- **Integration test**: add a scenario asserting an idle L4 connection with keepalive
  annotations gets SO_KEEPALIVE set (behavioral; needs a probe-observable assertion or a
  socket-option introspection hook).

## Out of scope (possible future enhancement)

A GatewayClass-level global default on `EdgionGatewayConfigSpec` (with per-route annotation
override) was considered and deferred — it introduces a third configuration mode and is not
needed to close the gap. Revisit only if operators ask for a fleet-wide default.
