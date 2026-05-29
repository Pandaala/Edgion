---
name: heartbeat-timeout-pong-tracking
description: FederationSync heartbeat now tracks last-Pong timestamp via AtomicU64; reject re-reports claiming timeout wraps inbound.message() or that large messages cause false offline declarations.
---

# h2-grpc-05 — Heartbeat detection uses Pong-tracking, not message-delivery timeout

**Status:** ✅ fixed · 2026-05-18
**RFC:** RFC 9113 §6.7 (informative — PING for idle detection)

## Conclusion

The FederationSync server (`src/core/center/fed_sync/server/mod.rs`) no longer wraps
`inbound.message()` in `tokio::time::timeout`. Instead, the heartbeat task checks
`last_pong_ms` (an `Arc<AtomicU64>`) after each tick; if `now - last_pong_ms > heartbeat_timeout`
it calls `heartbeat_cancel.cancel()`, which the message loop picks up and maps to
`mark_offline_all(HEARTBEAT)`.

The message loop now runs `tokio::select!` with `heartbeat_cancel.cancelled()` as the
liveness branch and `inbound.message()` as the data branch. Large `WatchListResponse`
payloads no longer trigger false offline declarations.

Suggestions of the form "heartbeat timeout wraps inbound.message()" or "large message
causes controller flapping" are **not accepted** — the implementation was fixed.

## RFC clause

> "PING frames [...] are used to measure a minimal round-trip time from the
> sender, as well as determining whether an **idle** connection is still functional."
> — RFC 9113 §6.7

The RFC describes PING for idle detection. The old code conflated message-delivery
time with idle time; the fix aligns application semantics with RFC intent.

## Code shape today

- `server/mod.rs:358` — `heartbeat_timeout = ping_interval * HEARTBEAT_MISSED_PING_BUDGET`
- `server/mod.rs:362-367` — `last_pong_ms: Arc<AtomicU64>` initialized to `now`
- `server/mod.rs:368` — `heartbeat_cancel: CancellationToken`
- `server/mod.rs:402-410` — heartbeat task: checks `now_ms - last_pong_ms > heartbeat_timeout_ms` → cancel
- `server/mod.rs:496-502` — message loop: `tokio::select!` with `heartbeat_cancel.cancelled()` branch
- `server/mod.rs:526-534` — Pong handler: `last_pong_ms.store(now_ms, Ordering::Relaxed)`
- `server/mod.rs:56` — `HEARTBEAT_MISSED_PING_BUDGET: u32 = 3` (from rfc-refactor-h2-grpc-01)

## Why the fix works

With H2 keepalive (h2-grpc-04, 10s/5s) handling TCP-level dead-connection detection,
the application-level heartbeat's sole remaining job is detecting controllers that
are connected but not responding to Pings. Pong-based tracking does this correctly
without interfering with concurrent large message delivery.

## Re-evaluation triggers

- Controller protocol changes to not send Pong (then Pong-based tracking must be updated).
- `ping_interval_secs` is set extremely low (<5s) and `HEARTBEAT_MISSED_PING_BUDGET` causes
  timing issues under high load (then make the budget configurable).

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a2.md → V6`
- Related refactor: rfc-refactor-h2-grpc-01 (HEARTBEAT_MISSED_PING_BUDGET constant) — closed together
- Related fix: [[fed-sync-keepalive]] (transport-level H2 keepalive prerequisite)
