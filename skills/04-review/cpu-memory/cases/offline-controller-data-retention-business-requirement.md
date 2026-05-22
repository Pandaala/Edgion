---
name: offline-controller-data-retention-business-requirement
description: Use when reviewing findings that flag ControllerRegistry or ResourceAggregator for not cleaning up offline controller entries as a memory leak; retention is a business requirement.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Offline Controller Data Retention is a Business Requirement

**False-positive scenario**: in `ControllerRegistry` / `ResourceAggregator`, entries for offline Controllers are not periodically cleaned up; flagged as a "memory leak".

**Reality**: registration info and resource snapshots of offline Controllers are **business data that must be persistently retained** — the Admin API needs to display offline state; operators need to view the resource snapshot before going offline. Cleanup goes through an explicit operations path: `DELETE /api/v1/center/admin/controllers/{id}` cascades to delete four in-memory structures + DB row (see the Admin API table in `skills/01-architecture/06-center/03-persistence.md`). Live resources (`stream_tx` / forwarding task / tonic HTTP/2 stream) are released immediately on `mark_offline`, not waiting for DELETE.

> Historical note: there used to be an `evict_stale(hours)` method + `offline_evict_hours` config, but it was never called by the scheduler — dead code. It was removed in 2026-04 fix #19; current semantics are "offline data has no TTL; cleaned only via Admin DELETE; but live resources are released the instant we go offline".

> **Important — boundary distinction (added 2026-05-16 after common-center-03 fix)**: this entry covers retention of *legitimately registered* controllers. It does **not** authorise accepting arbitrary attacker-supplied `RegisterRequest` shapes. The federation gRPC server defaults to plaintext and any TCP-reachable peer can submit a register, so `fed_sync::server::sync` performs shape validation + capacity gating before any state mutation (see `validate_register_req` / `registry_capacity_exceeded` in `src/core/center/fed_sync/server/mod.rs`). Current caps: `controller_id` ≤ 253 bytes (non-empty, no control chars), `cluster` ≤ 63 bytes (empty allowed → aggregator normalises to `"unknown"`), `env` / `tag` / `supported_kinds` ≤ 32 items × 63 bytes each, total registry ≤ 10_000 entries (reconnects of *known* ids are always allowed; only inflation by new ids is refused). Findings that propose adding a TTL/eviction sweep are still rejected by the retention rule above; findings that propose loosening / removing the boundary caps are also rejected — they are the defence-in-depth that complements `common-center-01`'s mTLS work. Treat the caps as fixed unless an operator can demonstrate a legitimate controller_id form that exceeds them.

**Verdict**: not a memory leak; it is business-semantics retention; live resources are released immediately; what is retained is only static registration snapshots.
