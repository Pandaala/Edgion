---
name: conf-sync-F003-async-write-lock-across-rpc
description: Use when reviewing findings that an `AsyncRwLock` / `AsyncMutex` write guard wraps a cloneable client (tonic `Channel`, `reqwest::Client`, ...) and is held across a full network round-trip — explains why "clone under brief lock, drop guard, then call" is the canonical fix and when the pattern is benign.
---

# AsyncRwLock Write Guard Held Across an RPC — Clone-and-Drop, Don't Re-derive

**Finding pattern**: a function takes `lock.write().await` on a `tokio::sync::RwLock<Option<Client>>` (or similar), obtains an `&mut Client` reference, and passes it to an `async` call that performs a network round-trip. The write guard lives across the entire I/O, blocking any other writer/reader of the lock for the RPC's duration. Often a nested sync lock (`parking_lot::RwLock`) is acquired inside the RPC body, establishing an `async-write → sync-write` ordering that becomes a deadlock if anything later acquires them in the opposite order.

**Verdict**: **real defect, even when no current caller triggers it.**

The lock there is needed only to read/clone the `Option<Client>`. The network I/O has no reason to happen under it. Tonic-generated clients wrap a `Channel` that is explicitly designed to be cheaply cloneable (it shares the underlying H2 connection pool); `reqwest::Client`, `aws_sdk_*::Client`, and most modern async client types are the same — clone is intended for exactly this pattern. The "right" critical section is two statements long: take the guard, clone, drop.

**Canonical fix**:

```rust
let mut client = {
    let guard = grpc_client.write().await;
    match guard.as_ref() {
        Some(c) => c.clone(),
        None => {
            drop(guard);
            // handle missing client
            continue;
        }
    }
};
// guard is dropped at end of the block above — the RPC below runs lock-free
let response = client.list(req).await?;
```

Notes:
- Use `as_ref()` (not `as_mut()`) so the guard can be downgraded after the clone — clone needs only `&Client`.
- The block scope (`let mut client = { ... }`) makes the drop point unambiguous; no manual `drop(guard)` needed on the success path.
- Even if `set_grpc_client` is currently only called once at init (so there's no concurrent writer today), the wide lock obscures intent and leaves a latent lock-ordering trap for any future reader path added to the same lock.

**When this is benign and should be left alone**:

- The client type is **not** cloneable (rare for modern async libraries; if you see it, that's a strong signal the lock should instead be `Arc<Client>` set once, not `Arc<RwLock<Option<Client>>>`).
- The lock truly protects mutation that must observe the RPC's effect atomically (e.g. "send request, parse response, update internal state — all visible to readers as one transition"). In practice this is rarer than reviewers assume; usually the post-RPC mutation can take its own short lock.
- The entire enclosing function runs at most once per process lifetime *and* the lock has no other waiter — purely cosmetic, but acceptable.

**Already applied**: `src/core/gateway/conf_sync/cache_client/event_dispatch.rs::start_watch` (2026-05-28, conf-sync F-003) — both the `list_and_reset` block and the watch block were refactored from `let mut client_guard = grpc_client.write().await; let client = client_guard.as_mut().unwrap();` (held across `list` RPC and the entire watch stream lifetime, respectively) to the clone-and-drop pattern above. `ConfigSyncClientService<Channel>` is the tonic-generated client and is cheaply cloneable.

**When to re-report**: only if a new code path holds an async write/read guard across a network call AND the client type cannot be cloned AND the lock has live readers/writers other than the current task. Pure init-only single-writer cases still deserve the cleanup but can be filed as `Severity: Low`, not as a bug.
