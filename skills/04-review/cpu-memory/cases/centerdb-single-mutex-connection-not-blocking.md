---
name: centerdb-single-mutex-connection-not-blocking
description: Use when reviewing findings that flag CenterDb single Mutex<Connection> as blocking admin and registration writes against each other; all DB ops are event-level and run in spawn_blocking.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# CenterDb Single `Mutex<Connection>` is Not a Write-Blocking Issue

**False-positive scenario**: `src/core/center/db/mod.rs:28-30`'s `CenterDb { conn: Arc<Mutex<rusqlite::Connection>> }` flagged as "shared single connection across the Center; admin list query and registration write block each other"; suggests introducing an r2d2 connection pool or write-connection + multiple read-connections + WAL.

**Reality**:

1. **Trigger frequency is event level / operations level, not request level**:
   - `upsert_controller` → Controller register/reconnect (per-Controller startup once, or occasional on network jitter)
   - `mark_offline` → heartbeat timeout / disconnection (sparse events at Controller granularity)
   - `delete_controller` / `list_controllers` → Admin API (manual operations)
   
   The data-plane request path does not touch this lock at all. In the extreme case of 1000 Controllers reconnecting simultaneously, a single upsert is ~ a few dozen microseconds; full serialization ≪ 1s.

2. **SQLite itself is single-write**: in the default rollback journal mode, multi-connection writes still serialize on the file lock. Switching to an r2d2 pool only moves the wait from Mutex to inside SQLite; zero benefit on the write side. WAL mode allows read non-blocking on write, but read frequency in this scenario (admin LIST) is also extremely low; benefit is unobservable.

3. **Write path already isolated by spawn_blocking**: `fed_sync/server/mod.rs:171, 307` and `api/mod.rs:248, 295` all run inside `tokio::task::spawn_blocking`; the Tokio runtime threads are not blocked by Mutex; the blocking pool defaults to 512 threads, more than enough for sub-ms operations.

4. **The code itself is positioned as best-effort**: the comment at `server/mod.rs:161-164` makes it clear: "Persist registration to SQLite (best-effort, isolated from the hot path). Any failure here is logged and swallowed — we refuse to block fed-sync registration on DB availability" — DB writes have never been on the critical path; failures are swallowed.

5. **Limited data scale**: only one table `controllers`; row count = Controller count (typically < 1000); all queries are by-PK single-row upsert/delete or full-table select.

6. **Change surface does not justify benefit**: r2d2 + WAL requires new dependencies, checkpoint coordination, test rewrites; profile does not show this lock at all.

**Verdict**: not a performance issue. Any "CenterDb single Mutex blocks / should switch to connection pool or WAL" finding is closed per this entry, unless it is proved that (a) DB operations have entered the request path, or (b) profile shows this lock has significant wait time.
