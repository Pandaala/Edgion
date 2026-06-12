---
name: access-log-store-ttl-read-path-cleanup
description: Decision record for observe F-006 — AccessLogStore TTL cleanup is now attached to both the write path (store) and the read path (list / status); a dedicated background timer was declined.
---

# observe F-006 — AccessLogStore TTL cleanup on read paths, no background timer

## Conclusion

`AccessLogStore::list()` and `AccessLogStore::status()` in
`edgion-gateway/src/observe/access_log_store.rs` now each begin with a call to
`self.cleanup_expired()`. The previous behavior — where eviction only happened
inside `store()` (every 100th call and on capacity overflow) — left expired
entries lingering in the `DashMap` until the next write, inflating
`status().entry_count` and pinning up to `DEFAULT_MAX_CAPACITY` (10_000) × ~1 KB
of memory in the integration-test process for up to `DEFAULT_TTL` (5 min) after
writes stopped.

Spawning a background tokio task from `AccessLogStore::new()` to run periodic
cleanup was **not** accepted.

## Core Rationale

**1. Scope is test-only.**

`AccessLogStore` is a `LazyLock<AccessLogStore>` consumed only when the gateway
is started with `--integration-testing-mode`; the only writer is
`pg_logging.rs` and the only readers are the `/admin/v1/access_logs/*` Admin
API handlers in `edgion-gateway/src/api/mod.rs`. Production binaries neither
write nor read this state, so any "memory leak" or `entry_count` skew is
bounded to the test process lifetime.

**2. Read-path cleanup closes the gap without new runtime surface.**

The defect surfaces precisely when reads outlive writes — exactly the moment
the read APIs are invoked. Calling `cleanup_expired()` at the head of `list()`
and `status()` evicts expired entries lazily on the very paths that would
otherwise observe stale state. `list()` already iterates the full map; the
added `retain` walk is the same O(N) order. `status()` becomes O(N) instead of
O(1), which is acceptable because it is an admin-only diagnostic call.

**3. A background timer adds runtime surface for no real gain.**

A `tokio::spawn`-ed periodic task inside `new()` (or at the gateway startup
path) requires a runtime handle, a shutdown signal, and lifetime coordination
with the global `LazyLock` — all just to keep a test-only `DashMap` tidy. The
read-path call achieves the same end state with two lines and no new task.
Test harnesses that do not poll `list()` / `status()` after their last write
are also the ones that immediately tear down the process, so any "leaked"
entries are reclaimed by process exit anyway.

## Fix Suggestions Not Accepted

- "Spawn a periodic `tokio::spawn` cleanup task from `AccessLogStore::new()`."
  — Adds runtime/shutdown plumbing for a test-only cache that the read paths
  already handle.
- "Replace the every-100-calls write-path cleanup now that reads also sweep."
  — Keep both. The write-path counter still bounds memory under heavy
  write-only workloads where reads might be infrequent or absent.
- "Drop `entry_count` from `AccessLogStoreStatus` since it can mislead." —
  `entry_count` is a useful diagnostic for test authors; the right fix is to
  make it accurate, which read-path cleanup does.

## Re-evaluation Triggers

Re-open this decision only if:

- A test scenario emerges that needs accurate `entry_count` / live memory
  without polling `list()` or `status()` (e.g., a long-running soak test that
  writes once and then idles for hours). In that case revisit a periodic task,
  scoped to the integration-testing mode flag.
- `AccessLogStore` is ever repurposed for a non-test path (production
  observability sink), at which point the whole TTL / capacity / eviction
  story needs a fresh design pass.

## Reference Cases

- Closed finding: `tasks/observe/observe-F006-access-log-store-ttl-no-cleanup-under-no-load.md` (closed 2026-05-28).
- Source: `edgion-gateway/src/observe/access_log_store.rs` — `list()`, `status()`, `cleanup_expired()`.
- Callers: `edgion-gateway/src/api/mod.rs` (Admin API `/admin/v1/access_logs/*`),
  `edgion-gateway/src/routes/http/proxy_http/pg_logging.rs` (writer).
