# Stream and TLSRoute synchronous plugin dispatch

Status: deferred optimization decision for the user.
Source baseline: ed2247fcb. No runtime or test change.

## Evidence

`plugins/stream/stream_plugin_runtime.rs` stores `Arc<dyn StreamPlugin>` and
awaits each `on_connection` in order, stopping at the first denial. The separate
`tls_route/tls_route_plugin_runtime.rs` does the same with `TlsRoutePlugin`.
Both contracts use `async_trait`. Cargo.lock pins async-trait 0.1.92; its
`src/expand.rs::transform_block` emits `Box::pin(async move { ... })`, and
`transform_sig` returns a pinned boxed dynamic Future.

StreamIpRestriction, GlobalConnectionIpRestriction, GeoIpLocation and both
fail-closed gates have no await in their callback bodies. The current TLSRoute
ACL also executes synchronously. These are not pure functions: snapshots,
local lookups, denial formatting and existing metrics can have costs or side
effects. ConnectionRateLimit genuinely awaits Redis; its local-only mode uses
the same asynchronous contract.

The bridge reaches this dispatch from Pingora ConnectionFilter before handshake;
TCP uses it per connection and UDP on new-session admission. TLS terminate and
passthrough share the distinct TLSRoute runtime. This is connection/session
work, not per HTTP request or response chunk. Source-level boxing is established;
actual optimized allocator traffic, CPU impact and throughput benefit have not
been measured. Compiler optimization and the larger admission costs matter.

## Decision and tradeoffs

- Keep the uniform async interface until representative profiling establishes
  a material cost. This retains the simplest extension contract.
- Investigate one canonical dispatch result with an immediate outcome or an
  asynchronous continuation. This may avoid wrappers for synchronous plugins,
  but adds branching, lifetimes and an execution-timing contract.
- Investigate a cold-built typed entry representation. It could expose static
  dispatch, at the cost of coupling runtime construction to concrete plugin
  types, representation size and additional maintenance when adding plugins.

Do not introduce a separate executor, listener scheduler, background admission,
or duplicate plugin evaluation. A no-work probe that itself evaluates policy
could repeat metrics, snapshots or token consumption. Moving callback effects
from the first future poll into construction can also change cancellation and
observation timing; absence of await does not prove that movement equivalent.

This task is distinct from [Redis compiled validation](redis-01-compiled-policy-validation.md).
The [HTTP body-hook profiling decision](../../skills/04-review/cpu-memory/intentional-design/async-response-body-hook-profile-before-gate.md)
concerns another API and workload; it supplies no measured connection-dispatch
benefit. The accepted [serial admission policy](../../skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md)
remains in force. Its historical fork revision is not the current dependency pin.

## Acceptance before implementation

Preserve ordered exactly-once evaluation, first-denial short-circuiting, disabled
and Bypass handling, Block and invalid-source fail-closed gates, source-generation
cache identity, captured publication ownership, and per-plugin snapshot timing.
Retain Stage 1 physical-peer semantics, separate TLSRoute context and both TLS
proxy modes, UDP session reuse, and cumulative listener/route attachments.
Redis keeps local-token-first admission without refund, listener-before-source
ordering, current client resolution, key scope, deadlines and failure policies.
Preserve Send/lifetime requirements and cancellation/drop behavior; do not hold
new locks across waits or change denial strings, metrics or protocol logs.

Measure real optimized dispatch with empty, one-entry and multi-entry chains,
allow and early/late denial, local-only and Redis-backed admission, and both TLS
modes. Separate per-plugin wrapper allocations from the outer Pingora filter,
denial strings, matcher work and network latency. Report CPU, allocation counts,
retained size and representative throughput/tail latency without new permanent
metrics or tracing. Test order and side-effect counts, cancellation before first
poll and during Redis waits, invalid configurations and hot-reload snapshots.

Original architecture: [Stream plugin development](../../skills/01-architecture/02-gateway/13-stream-plugin-dev.md).
This task records source inspection only. No candidate implementation, runtime
test, allocator measurement or live-network benchmark accompanies it.
