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
resource map and resolved runtime map in the same `ArcSwap` generation. A
listener's `GatewayConfigHandle::load` accessor captures one Arc from the listener's stable
binding without a name lookup. Publication updates that handle on EGC changes.
The connection retains that Arc and copies its `l4_timeouts` for the relay. The
duration parser and blocking write lock stay on the configuration update path.

The former separate resolved-timeout map is now part of the unified
[EGC runtime snapshot](egc-runtime-snapshot-publication.md); the timeout lifetime
and defaults below are unchanged.

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

Historical timeout-fix verification: targeted unit tests covered partial update and removal, authoritative empty full
sets, missing/default behavior, a real EGC named `default`, independent configs,
mixed invalid and zero fields, connection-copy stability, and release of an old
parent snapshot. The targeted command
`cargo test -p edgion-gateway l4_timeout_ --lib` passed all three selected tests.
This historical result does not establish fresh verification of later unified
EGC publication changes.

The dynamic scenario `L4TimeoutHotReload`
(`edgion-tests/src/client/dynamic/scenarios/l4_timeout_hot_reload.rs`) is the
wire-level hot-update guard. It uses a dedicated GatewayClass and
`EdgionGatewayConfig` named `l4-timeout-egc` so the shared `example-gateway`
`idleTimeout: 2s` baseline is unchanged. After a generous idle timeout is
published, it completes a backend exchange on live TCP, TLS-terminate, and TLS
passthrough connections, updates only `spec.tcpTimeout.idleTimeout` to `1s`,
and waits for two observable signals: Gateway `configclient` showing the new
string, then a disposable TCP probe whose idle close matches the new generation.
`configclient` is ClientCache receipt and can precede `CacheData::BATCH_DEBOUNCE`
(100ms) plus ConfHandler publication; the probe is what proves
`ResolvedL4Timeouts` was published. New
connections on all three paths must then close inside the short window, while
the pre-update connections remain usable. Fixed sleeps are used only for the
idle interval itself. The scenario would fail against the former startup-frozen
timeout behavior. The Kubernetes fixture and execution registration are tracked
by [Kubernetes dynamic scenario inventory parity](../../testing/regression-guards/kubernetes-dynamic-scenario-inventory-parity.md);
the Kubernetes scenario was not run against a live cluster in that parity change.

`connectTimeout` is published and captured through the same resolved snapshot.
This scenario does not add connect-timeout wire coverage: holding upstream
connection establishment open without OS or network black-hole timing is not
deterministic across Linux CI and macOS. Resolver, publication, and
connection-copy unit tests remain the accepted connect-timeout coverage
boundary.

Re-evaluate when connection acceptance ownership moves, the physical listener's
EGC reference becomes dynamically retargetable, another L4 proxy is introduced,
a timeout must change an already established connection, or a deterministic
cross-platform connect-timeout fixture becomes available.

## Resolution references

- `pre-resolve-l4-listener-timeouts` — corrected the startup-frozen policy and
  per-connection string parsing by publishing resolved dynamic snapshots; later
  closed the remaining transport hot-reload integration gap as
  `L4TimeoutHotReload`.
