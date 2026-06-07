# listener-03: Downstream connection limit & overload protection

**Source review finding**: `tasks/review-envoy/listener-03-no-downstream-connection-limit.md`
(promoted to a task on 2026-05-30 via fix-review-issue Scope gate option A — too wide for the
narrow review-fix workflow).

**Severity**: high · **Verdict**: ✅ real gap (defense-in-depth layer missing) · **Value**: 🟢 worth fixing

## Problem summary

Edgion has **no** per-listener or global downstream connection admission control. The only
related knob is `downstream_keepalive_request_limit` (requests *per connection*, default 1000,
`src/core/gateway/runtime/server/listener_builder.rs:178-195`), which does not bound the *number*
of simultaneous connections. `grep` confirms no `max_connections` / `global_downstream` code exists.

`StreamPluginConnectionFilter` (`src/core/gateway/plugins/stream/connection_filter_bridge.rs`) can
reject connections at the TCP layer, but it is opt-in (requires an `edgion.io/edgion-stream-plugins`
annotation with IP allow/deny rules) and is not a system-level safety net.

Consequence: a connection flood (DDoS / traffic spike / misbehaving client) can exhaust OS file
descriptors or Tokio per-task memory, causing the whole process to fail `accept()` — taking down all
listeners on the same process (including internal gRPC and admin API ports), not just the flooded one.

## Envoy baseline (reference)

1. `global_downstream_max_connections` — process-wide cap; over-limit connections get an immediate RST.
2. Per-listener limits (`per_connection_buffer_limit_bytes`, transport-socket throttling).
3. Overload Manager — monitors heap / fd / CPU and applies graduated load-shedding actions.

## Suggested direction (staged — to be designed via brainstorming)

- **Near-term**: optional `max_downstream_connections` field on `EdgionGatewayConfigSpec` / `ServerConfig`;
  a global atomic counter incremented on accept and decremented on connection close; reject (TCP RST)
  when over the limit. Default = unlimited (preserve backward compatibility).
- **Medium-term**: expose current downstream connection count as a Prometheus gauge
  (`edgion_downstream_connections_total`) plus a "connections rejected due to limit" counter.
- **Longer-term**: investigate Pingora backlog tuning and a listener-level limit at the `Service`
  level to avoid per-request overhead.

## Design questions to resolve before coding

- Global counter vs per-listener counter (or both) — and where the increment/decrement hooks live
  relative to Pingora's `ConnectionFilter` / `Service` lifecycle (avoid per-request overhead).
- Counter decrement correctness on every connection-close path (graceful, error, TLS-handshake-fail).
- Metric naming and cardinality (per-listener label vs global).
- Sensible default and how it interacts with OS `ulimit -n` / container fd limits.
- Whether the atomic on the accept hot path is acceptable, or a relaxed/sharded counter is needed.
