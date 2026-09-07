---
name: l4-relay-idle-timeout
description: Use when reviewing L4 relay findings; covers the landed idle timeout that bounds all three L4 proxies — do not re-report "no L4 relay idle timeout".
status: fixed
finding_id: fr-l4-02
closed: 2026-06-02
knowledge_type: regression-guards
---

# L4 Relay Idle Timeout — NOT-AN-ISSUE

## Conclusion

The shared L4 relay `bidirectional_relay` (`edgion-gateway/src/routes/l4_duplex.rs`) is
bounded by an idle timeout for all three proxies (plain TCP, TLS-terminated,
TLS-passthrough).

Fix suggestions of the form "TCP/TLS L4 relay has no idle timeout / stalled peer holds
the relay open indefinitely" are **not accepted**.

## Core Rationale

**1. Single idle clock covers both halves of the relay**

A single idle timer is reset on any byte progress — completed read OR any advancing
partial write via `write_all_idle` / `flush_idle`. This bounds both read-stall and
write/receive-window stall without requiring separate per-direction timers.
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

## Fix Suggestions Not Accepted

- "Add per-proxy idle timeout logic" — the centralized helper already enforces it; per-proxy duplication would reintroduce the code-duplication problem that was cleaned up (see `l4-duplex-relay-centralization.md`).
- "Bound determined trickle attackers (1 byte/window resets the clock)" — this is by design; a total-duration cap would sever legitimate long-idle connections. If needed, this is a new opt-in feature, not a bug fix.
- "Apply `close_notify` on idle timeout closure" — idle closure is intentionally abortive, consistent with all other error-path breaks in the relay.

## Re-evaluation Triggers

Re-open this decision only if:

- A new L4 proxy type is added that does NOT route its relay through `bidirectional_relay` (it would lack the protection).
- A request arrives to bound trickle attackers or add a total-duration cap (that is a new feature request, not a re-report of this finding).

## Reference Cases

- Spec: `docs/superpowers/specs/2026-06-02-l4-relay-idle-timeout-design.md`
- Plan: `docs/superpowers/plans/2026-06-02-l4-relay-idle-timeout.md`
- Architecture note: `skills/04-review/routing/regression-guards/l4-duplex-relay-centralization.md`
- Implementation: `edgion-gateway/src/routes/l4_duplex.rs`
