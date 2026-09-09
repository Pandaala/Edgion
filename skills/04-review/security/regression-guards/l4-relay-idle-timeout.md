---
name: l4-relay-idle-timeout
description: Shared L4 relay idle deadline, byte-progress resets, explicit opt-out, and backpressure verification boundaries.
status: fixed
finding_id: fr-l4-02
closed: 2026-06-02
knowledge_type: regression-guards
---

# L4 Relay Idle Timeout

## Conclusion

The shared L4 relay `bidirectional_relay` (`edgion-gateway/src/routes/l4_duplex.rs`)
enforces the configured idle timeout for plain TCP, TLS-terminated, and
TLS-passthrough proxies. The historical fr-l4-02 correction established this
clock; it did not establish independent progress under write backpressure.
Current findings must verify the affected operation and progress semantics,
not infer correctness from the historical closed status.

## Core Rationale

**1. Single idle clock covers both halves of the relay**

A single idle timer is reset on any byte progress — completed read OR any advancing
partial write. Each direction retains its own read/write/flush state and is
polled in the same relay future. This bounds read stalls and stalled receive
windows without separate per-direction timers. Pending flush has no byte-progress
signal, but progress in the reverse direction resets the shared deadline.
Flush completion itself does not reset it. `None` disables timer polling.
`DuplexError::IdleTimeout` surfaces as `TcpStatus::IdleTimeout` for the TCP proxy and
as an `"idle timeout"` `err_log` entry for TLS proxies.

**2. Secure-by-default with explicit opt-out**

`resolve_tcp_idle_timeout(spec.tcpTimeout)` returns 1 hour even when no `tcpTimeout`
section is configured. Only an explicit `idleTimeout: "0s"` (any zero-valued
GEP-2257 duration; a bare `"0"` is rejected at admission) opts out. Operators who
legitimately need unbounded connections must consciously disable the timeout.

**3. All three L4 proxy types route through the same helper**

Plain TCP (`edgion_tcp.rs`), TLS-terminated (`tls/proxy.rs`), and TLS-passthrough
(`tls/passthrough_proxy.rs`) all call `bidirectional_relay`. The protection cannot be
selectively absent from one proxy variant.

## Scope and non-goals

- Per-proxy idle clocks would duplicate the shared mechanism; change the canonical relay instead (see `l4-duplex-relay-centralization.md`).
- "Bound determined trickle attackers (1 byte/window resets the clock)" — this is by design; a total-duration cap would sever legitimate long-idle connections. If needed, this is a new opt-in feature, not a bug fix.
- "Apply `close_notify` on idle timeout closure" — idle closure is intentionally abortive, consistent with all other error-path breaks in the relay.

## Backpressure correction and verification

B0196 replaced whole-direction write/flush awaits with independent poll states.
The earlier shape timed out stalled connections but could not forward reverse
ACKs needed to unblock the writer. Two deterministic in-memory ACK regressions
failed on that implementation; the correction preserves the single idle clock.

Paused-clock unit tests assert no early expiration, exact expiration after the
last byte progress, partial-write survival beyond the original window, reverse
activity during a blocked write/flush, idle reads, and explicit unlimited waits.
Error and EOF tests distinguish one timeout/error hook call from clean closure.
These replace the earlier helper-only write tests with relay-level coverage.
They do not establish transport-level integration behavior. The
`add-l4-duplex-backpressure-integration` follow-up adds a shared-topology suite;
its current results and network-versus-poll-state limits are maintained in
[the shared relay regression boundary](../../routing/regression-guards/l4-duplex-relay-centralization.md#transport-level-regression-boundary).

## Re-evaluation Triggers

Re-evaluate when:

- A new L4 proxy type is added that does NOT route its relay through `bidirectional_relay` (it would lack the protection).
- A request arrives to bound trickle attackers or add a total-duration cap (that is a new feature request, not a re-report of this finding).
- Relay polling, idle resets, partial-write cursors, pending flush behavior, or
  stream-wrapper readiness changes; an existing timeout does not disprove a
  newly identified progress or wakeup defect.

## Reference Cases

- Spec: `docs/superpowers/specs/2026-06-02-l4-relay-idle-timeout-design.md`
- Plan: `docs/superpowers/plans/2026-06-02-l4-relay-idle-timeout.md`
- Architecture note: `skills/04-review/routing/regression-guards/l4-duplex-relay-centralization.md`
- Implementation: `edgion-gateway/src/routes/l4_duplex.rs`
