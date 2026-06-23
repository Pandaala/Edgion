---
name: consistent-hash-ring-seenset-no-heap
description: Use when reviewing ConsistentHashRing::select per-request HashSet allocation findings — select now uses a stack u128 bitmask / Vec<bool> SeenSet instead of HashSet::new() per call.
---

# ConsistentHashRing::select Per-Request HashSet Allocation

**Status / verdict**: fixed (lb-F003, 2026-05-24)

When reviewing findings of the form "`ConsistentHashRing::select` allocates a `HashSet` on every request / per-request heap allocation on the ConsistentHash LB hot path / `seen` set grows dynamically during the ring walk (lb-F003)": **fixed (lb-F003, 2026-05-24)**. `select` (`edgion-gateway/src/lb/selection/consistent_hash.rs`) now tracks visited backend indices with a private `SeenSet` enum — stack `u128` bitmask for `backend_count <= 128` (zero heap on the common O(10-100) path), single `Vec<bool>` fallback above that — instead of `HashSet::new()` per call. Backend indices are dense (`0..backends.len()`) so the bitmask shift is statically `< 128`. Behavior-preserving: `seen` is a pure dedup optimization, the selection result and termination are unchanged. Reached every ConsistentHash request via `ChCore::select` (`compiled_selector.rs`, `SELECT_MAX_ITERATIONS = 256`); the P2C sibling was already allocation-free. Unit regression `large_backend_count_uses_vec_seen_path`. Re-reports of the per-request HashSet allocation are not accepted; verify `select` uses `SeenSet` before re-reporting.
