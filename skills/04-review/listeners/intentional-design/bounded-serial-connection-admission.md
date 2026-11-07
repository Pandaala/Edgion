---
name: bounded-serial-connection-admission
description: Review Stage 1 Redis connection limiting, accept serialization, and the accepted scope of local and distributed admission.
knowledge_type: intentional-design
---

# Serial connection admission with optional Redis limits

## Conclusion and scope

D0099 / `avoid-listener-accept-head-of-line-blocking` is closed by explicit user
acceptance of the current performance tradeoff, not by a runtime fix or a
false-positive finding. Keep local and Redis connection limiting available in
combination. Do not introduce a new Pingora admission scheduler for this issue.
This decision concerns Gateway-attached Stage 1 filters; it is not acceptance
of unrelated listener defects or a guarantee of sufficient capacity.

Routine burst protection can use local `perListener` limiting. Shared limits
remain opt-in when their semantics justify dependency latency. HTTP business
quotas generally belong at request scope rather than connection scope.

## Rationale and evidence

The inspected Cargo.lock pins Edgion Pingora `edgion_v0.9.1` at
`e5f4287be713d01ffd444544251bf75c9d00062a` (re-verified by source inspection on
2026-11-06; originally recorded at `edgion_v4@518f7274`). Its call chain is
`Service::run_endpoint` -> `TransportStack::accept` ->
`ListenerEndpoint::accept` -> `ConnectionFilter::should_accept().await`.
(Current fork: `ListenerEndpoint::accept` calls `ConnectionFilter::admit`, which
may attach a connection permit to the accepted stream; Edgion's listener filter
counts the socket before running the Stage-1 chain. See
[downstream connection admission](../regression-guards/downstream-connection-admission.md).)
Only after filtering returns does the service spawn the handshake task.
`ServerConf.listener_tasks_per_fd` defaults to one and controls the number of
parallel accept tasks. Each such task holds at most one socket undergoing
filtering; later sockets queue in the kernel backlog. This is not a bound on
all established connections or handshake tasks. The 60-second handshake
timeout starts after filtering. The service selects between accept and
shutdown, so shutdown can cancel the pending filter future.

Edgion's `runtime/server/listener_builder.rs::apply_connection_filter` installs
`ListenerAdmissionFilter` (`runtime/server/downstream_connections.rs`): the
fatal PROXY-policy guard, then the process connection cap and listener slot,
then the stream bridge.
The bridge captures one immutable `PluginPublication` snapshot and obtains the
physical listener's `ConnectionView`. Publication compares the effective views
of every serving Gateway; disagreement or unavailable policy yields
`Unavailable` and fails closed, while `Ready` pins one `CompiledStreamPlugins`
Arc for the admission. `listener_port` supplies the port to `StreamContext`.
The peer is the physical pre-PROXY `DirectPeerIp`; IPv4-mapped IPv6 is folded to
canonical IPv4 before evaluation without changing that ownership. Preserve this
ordering, snapshot scope, configured failure policy, and the prohibition on
per-connection tracing.

`plugins/stream/connection_rate_limit/plugin.rs::on_connection` evaluates:

1. Local `perListener` token bucket whenever that dimension is configured.
2. Redis `perListener` only when `redisDistributed` is true.
3. Redis `perSourceIp` when configured.

Any denial stops the chain. Distributed listener limiting retains the local
gate. `redis_dimension` immediately applies failure policy for a missing or
unhealthy client; otherwise `redis_timeout` bounds each dimension (default
100ms, runtime cap 1s). Multiple dimensions and entries can accumulate waits.
`pl_on_redis_failure` and `ip_on_redis_failure` retain their configured meaning;
fail-open does not imply zero wait. `redisRef` is required only for Redis-backed
dimensions. No configuration defaults or runtime behavior changed here.

## Alternatives and limits

Local admission reduces Redis load but does not eliminate dependency waits for
connections that pass. Raising the process FD limit or Redis pool capacity
does not remove accept serialization. Pingora's existing accept parallelism
can mitigate it, while increasing concurrent Redis work; this review does not
claim Edgion exposes that setting as an operator knob or prescribe a value.

A new bounded scheduler could isolate slow admissions more fully, but would
introduce socket ownership, cancellation, shutdown and concurrency semantics
across the fork boundary. The user accepted the existing limitation after
reviewing these costs and the local-plus-Redis composition. No workload test
has established a need for that redesign. Saturation can still delay or lose
new connections; the performance issue remains real.

Envoy also supports local plus external connection limiting, but its network
rate-limit filter pauses a connection's filter chain with `StopIteration` and
resumes via `continueReading`. This is evidence of a useful feature, not proof
that Edgion's accept scheduling is equivalent:
[global limiting](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/other_features/global_rate_limiting),
[filter implementation](https://github.com/envoyproxy/envoy/blob/main/source/extensions/filters/network/ratelimit/ratelimit.cc).

## Verification and re-evaluation

Current verification is source inspection and bilingual documentation review;
no deterministic slow-filter reproduction, benchmark, or integration test was
run for this decision. Independent read-only review returned LGTM after
corrections to the architecture's bucket scope and lifecycle terminology.
`cargo fmt --all`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` completed successfully; Clippy emitted
warnings in unchanged Rust sources. The agent-document consistency, SSA force,
metrics inventory, and Pingora source policy guards all passed. No Rust or
existing integration-test files changed; full tests were not requested or run.
Historical B0147 test results do not establish current throughput.

The separate `clarify-stream-plugin-udp-architecture-coverage` documentation task
reconciled the architecture wording with the
[documented UDPRoute wiring](../../routing/regression-guards/udproute-stream-plugins-annotation-wiring.md).
That correction preserves this decision's narrower scope: Gateway-attached
Pingora `ConnectionFilter` admission does not cover UDP listeners, while a
UDPRoute attachment runs the same `spec.plugins` runtime at new-session admission.
No UDP runtime or integration-test behavior changed.

Re-evaluate if a measured connection-rate target cannot be met with the existing
mechanisms, dependency stalls cause an operator incident, another blocking
Stage 1 plugin is added, or Pingora changes accept ownership or cancellation.
Any future scheduler design must first reproduce the limitation and cover
saturation, shutdown, task failure, and preserved security ordering.

## Resolution references

- D0099: originally deferred by B0147 on 2026-09-08; accepted performance
  limitation with documentation-only closure in this user-directed review.
- Operator guidance: `docs/en/user-guide/tcp-route/stream-plugins.md` and its
  `docs/zh-CN/` counterpart, ConnectionRateLimit section.
