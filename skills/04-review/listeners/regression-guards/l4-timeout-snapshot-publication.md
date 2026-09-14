---
name: l4-timeout-snapshot-publication
description: Use when reviewing EdgionGatewayConfig tcpTimeout hot reload, connection-generation pinning, or proposals for separate TCP and TLS relay timeout configs.
knowledge_type: regression-guards
---

# L4 timeout snapshot publication and connection pinning

## Conclusion and scope

`EdgionGatewayConfig.spec.tcpTimeout` updates apply to subsequent plain TCP,
TLS-terminated, and TLS-passthrough connections without rebuilding their
listeners. The Gateway parses both duration strings while publishing the
immutable EdgionGatewayConfig runtime snapshot. Each connection copies one
`ResolvedL4Timeouts` value when it enters its L4 proxy `process_new` path and
keeps that generation through upstream connect attempts and the relay.

An in-flight connection never switches timeout generations. For TLS termination,
Pingora has completed the downstream TLS handshake before the application
`process_new` callback runs; this rule does not claim raw-socket accept-time
pinning. UDP, downstream handshake deadlines, passthrough ClientHello deadlines,
setup writes, and a retry-sequence total budget are separate mechanisms.

## Rationale and evidence

`config/edgion_gateway/conf_handler_impl.rs::EdgionGatewayConfigSnapshot` owns the
resource map and resolved timeout map in the same `ArcSwap` generation. Its
`get_resolved_l4_timeouts` accessor performs one lock-free snapshot load, one
bounded lookup by the listener's startup-selected EGC name, and a `Copy`. The
duration parser and blocking write lock stay on the configuration update path.

`runtime/server/base.rs` preserves `None` for a GatewayClass with no
`parametersRef`; it must not infer the reference from the synthetic default
resource name. A missing or deleted referenced EGC resolves to the canonical
10-second connect and one-hour idle defaults. Valid zero durations remain
explicit unlimited values, and invalid transported strings independently use
the existing fail-safe resolver fallback.

The three consumers are:

- `routes/tcp/edgion_tcp.rs`
- `routes/tls/proxy.rs`
- `routes/tls/passthrough_proxy.rs`

Both TLS retry loops reuse the connection copy rather than loading again. All
three relays pass its idle member into the canonical `bidirectional_relay`.

## Alternatives and limits

Resolving only during listener construction is cheaper by one snapshot lookup
but violates the runtime EGC update contract. Parsing raw strings per connection
supports updates but leaves configuration work on the hot path. Updating an
in-flight relay would require deadline migration semantics and could change one
connection between retry attempts; that broader behavior is intentionally absent.

TCP and both TLS modes share this configuration because their upstream connects
are ordinary TCP and their established byte streams use the same relay. A future
TLS-specific handshake, ClientHello, setup-write, or total-retry deadline should
use a field named for that distinct operation rather than duplicate the shared
connect and idle fields.

The EGC name belongs to the physical listener selected at startup. This change
does not dynamically retarget a GatewayClass `parametersRef`, transfer a shared
listener to another Gateway, add listener ports, or support Gateway deletion and
recreation. Those remain listener-lifecycle boundaries.

## Verification and re-evaluation

Targeted unit tests cover partial update and removal, authoritative empty full
sets, missing/default behavior, a real EGC named `default`, independent configs,
mixed invalid and zero fields, connection-copy stability, and release of an old
parent snapshot. The targeted command
`cargo test -p edgion-gateway l4_timeout_ --lib` passed all three selected tests.
No new transport integration scenario was added, so wire timing remains covered
by the existing connect and relay suites rather than a fresh hot-update test.

Re-evaluate when connection acceptance ownership moves, the physical listener's
EGC reference becomes dynamically retargetable, another L4 proxy is introduced,
or a timeout must change an already established connection.

## Resolution references

- `pre-resolve-l4-listener-timeouts` — corrected the startup-frozen policy and
  per-connection string parsing by publishing resolved dynamic snapshots.
