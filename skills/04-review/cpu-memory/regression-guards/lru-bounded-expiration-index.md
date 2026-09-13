---
name: lru-bounded-expiration-index
description: Use when reviewing LocalLruCache expiration maintenance; records that a BTreeSet deadline index replaced the full-list prune scan, and which alternative designs are rejected and on what grounds.
status: fixed
finding_id: task-bound-lru-expiration-maintenance
closed: 2026-09-03
knowledge_type: regression-guards
---

# LRU Expiration Maintenance Is a Bounded Ordered Index — FIXED

## Conclusion

`LocalLruCacheInner` carries a `BTreeSet<(Instant, usize)>` deadline index holding exactly one
record per live TTL entry. A full-cache insert consults the earliest record and stops there, so
insertion cost no longer depends on capacity. The former `prune_expired_from_tail` full-list walk
is gone.

Fix suggestions of the form "sort the LRU list by deadline and binary-search it", "add a background
expiry task or timer", or "prune on every insert" are **not accepted**. A *lazy versioned* heap is
also not accepted. Note carefully that this is **not** a rejection of every heap — see the
alternatives table, which records why an indexed heap is a legitimate design that lost on
cost/benefit rather than on correctness.

## Core Rationale

**1. The defect was a capacity-linear scan with no early exit**

`insert` called `prune_expired_from_tail(now)` whenever `free_head.is_none()`, and that function
walked the entire LRU list from `tail` testing `is_expired` on every node. With nothing expired —
the steady state of a warm cache — the walk produced nothing and still cost O(capacity) while
holding the shard mutex.

Measured on the `live` workload with `edgion-gateway/benches/lru_expiration.rs`, per-insert p50:

| capacity | before | after |
|---|---|---|
| 1K | 14.2 us | 1.54 us |
| 16K | 204 us | 1.63 us |
| 256K | 3.32 ms | 1.96 us |

The before column is linear in capacity across two independent decades (16x capacity for 14.4x
time, then 16x for 16.3x); the after column is flat within run-to-run noise (a 256x capacity
increase costs 1.16-1.32x the time, measured across repeat runs). **The load-bearing claim here is
the shape, not the absolute values.**

Two caveats a future reader must not skip:

- These are **dev-profile (unoptimized) numbers on aarch64/macOS**, chosen to avoid a release build.
  Dev-profile absolute values overstate release cost by roughly an order of magnitude and are
  **not** a per-request CPU budget input. Only the capacity-scaling ratios carry over. The
  qualitative claim does not depend on the profile: the old code was an unconditional full-list
  walk, which is capacity-linear in any profile, and the 256K gap is ~1700x — optimization changes
  the constant, never the exponent, and would in fact widen this particular ratio (the linked-list
  walk is memory-latency-bound where optimization helps least).
- The before column was produced by temporarily restoring the old walk as `prune_expired`'s body
  while leaving `remove_index`'s index maintenance in place. That hybrid never shipped; it charges
  the old algorithm for BTreeSet work it never did, a bias *against* the old code of <0.1% at
  16K/256K. To reproduce faithfully, check out the commit preceding this change and run the bench
  against its `prune_expired_from_tail`, and scale `SEED_TTL` first (see that constant's comment).

**2. One list cannot carry two orders**

`head`/`tail`/`prev`/`next` express recency: `touch` moves a node to the head and `alloc_index`
evicts from the tail. TTLs are per-entry (`insert(key, value, ttl)`), so insertion order, access
order, and deadline order are mutually unrelated. Reordering that list by deadline deletes LRU
semantics. Binary search is separately impossible: it needs O(1) access by rank, and a linked list
offers only sequential traversal, so "binary search" degenerates into the same O(n) walk. The node
pool `Vec` cannot be sorted either — `map: HashMap<K, usize>` and every `prev`/`next` link store
slot indices, so reordering the pool invalidates all of them.

**3. Point removal is what makes the index exact**

A `BTreeSet` keyed by `(deadline, node index)` supports removal of a specific record, so the two
paths that invalidate a deadline can drop the old one precisely: a TTL rewrite in `insert`'s update
branch (via `sync_expiration_record`) and any release through `remove_index`. The index therefore
contains only live records and needs no version field, no stale-record validation, no compaction
threshold, and no amortization argument.

**Record count is bounded by capacity** — that is the tested, load-bearing bound
(`test_repeated_updates_keep_expiration_index_bounded` holds it across 10,000 rewrites of one key).
**Bytes are not `24 * capacity`.** `(Instant, usize)` is 24 bytes on every supported target
(`Instant` is a 16-byte `Timespec` on Unix), but a `BTreeSet` packs up to 11 elements into a
280-byte node, and ascending deadlines — the real shape, since the OIDC caches insert with a
constant TTL — split nodes and never refill the left half, settling near 50% occupancy. Measured
with a counting allocator: **~49 bytes/entry**, or ~56 bytes of RSS once jemalloc's size classes
round up. So ~50 KiB at the 1024-entry default and ~12 MiB at the 65,536-entry configured ceiling,
against a cache that already reserves ~186 bytes/slot of empty skeleton and can hold tens of MiB of
tokens. Treat `24 * capacity` as a payload floor, never as a bound.

**4. Expiry stays lazy and request-driven**

The index is passive: it triggers nothing on its own. Reclamation happens only at the three
pre-existing call sites — `get` on an expired hit, `insert` when the cache is full, and explicit
`remove`. An idle cache consumes no CPU. No timer, no tokio task, no timer wheel was added.

**5. Observable semantics are unchanged; the update path costs slightly more**

LRU replacement, TTL, explicit removal, borrowed-key lookup, fixed capacity, and all four counters
behave as before. `prune_expired` still drains *every* expired entry before `alloc_index` may
displace a live one, preserving the contract that a slot freed by expiry is not counted as an
eviction. The only API addition is `expiration_index_len()`, a `#[doc(hidden)]` diagnostic accessor
for tests and benchmarks.

The honest cost: a TTL rewrite now pays a `BTreeSet` remove plus insert where it previously paid
nothing, and `get` on an expired entry pays an O(log n) removal. The `update_heavy` benchmark
workload exists to watch this; its baseline is p50 1.08 us / 1.13 us / 1.38 us at 1K / 16K / 256K
(same dev-profile caveat).

## Alternatives Considered

| Alternative | Verdict | Why |
|---|---|---|
| Sort the LRU list by deadline / binary-search it | Rejected on correctness | The list already encodes recency (Rationale 2); a linked list cannot be binary-searched. |
| Deadline-sorted `Vec` + binary search for the insertion point | Rejected on correctness of the claim | The search is O(log n) but the ensuing memmove is O(capacity), so the capacity-linear cost survives with a smaller constant. |
| Lazy versioned min-heap (the originating task's proposal, citing Traefik's ttlmap) | Rejected on complexity | No point removal, so stale records accumulate at the TTL-rewrite rate, which is unbounded relative to capacity. Requires a version field, pop-time validation, a compaction threshold, and an amortization argument — all of which the exact index simply does not need. |
| **Indexed (position-tracked) binary heap** — `Vec<(Instant, usize)>` preallocated to capacity plus a `heap_pos` field on `CacheNode`, with sift-up/sift-down on removal | **Rejected on cost/benefit, NOT on correctness** | Semantically equivalent to the `BTreeSet`: exact point removal, no version field, no stale records, no compaction. Strictly better on allocation — contiguous, cache-friendly, allocated once, and it would preserve this type's otherwise-complete "allocate at construction" property, which the `BTreeSet` does break. It loses only because ~40 lines of hand-written sift logic is real bug surface, traded against an allocation jemalloc serves from its thread cache in tens of nanoseconds, at a configured ceiling of 65,536 entries. **A re-report proposing this is a legitimate suggestion, not a repeat of a settled question** — weigh it on those terms rather than dismissing it as "a heap". |
| Coarse-granularity deadline buckets (`BTreeMap<coarse_instant, SmallVec<idx>>`) | Rejected | Amortizes toward O(1) and is safe (exact deadlines are still checked in `get`, so coarsening only delays slot reclamation), but removing one index from a bucket on the TTL-rewrite path needs a scan or tombstones, and it buys nothing at a default capacity of 1024. |
| One FIFO per distinct TTL value (memcached/nginx style) | Rejected | Zero-allocation over the existing node pool and correctly sorted *within* a TTL class, but correctness of the "earliest deadline" query depends on TTL cardinality staying small, which the `insert(key, value, ttl)` API does not enforce. A single intrusive TTL list is worse still — it is sorted only if all entries share one TTL, otherwise insertion degenerates to an O(n) position search. |
| Hierarchical/hashed timer wheel | Rejected | Not because it implies a background task — a hashed wheel can be advanced lazily on the request path. It loses because it has no exact "earliest deadline" query, needs cascading logic, and sizes its bucket array by time horizon rather than by capacity. |
| Do nothing; rely on the capacity ceiling | Rejected | The ceiling is `MAX_LRU_CACHE_CAPACITY = 65_536` total across shards with a default of 1024, so the real pre-fix worst case was a ~65K-node walk and the typical case ~1K — smaller than the benchmark headline, but still a full scan under the shard mutex on a path reachable per request. |
| Add a third shared cache implementation, or move this cache onto TinyUFO | Rejected | Forbidden by the in-process cache selection rule (`AGENTS.md` / `CLAUDE.md`, "In-process cache selection"); this cache exists precisely because explicit invalidation and strict replacement are required. |
| Prune while a free slot already exists | Rejected | `alloc_index` returns immediately while `free_head` is `Some`, so pruning there is pure overhead. |

## Invariant To Check On Re-Review

The index and the node pool must never diverge. Every path that assigns, rewrites, or releases a
deadline updates both in the same step:

| Path | Index obligation |
|------|------------------|
| `insert` on an existing key | `sync_expiration_record` drops the old record, adds the new one **at the new deadline** (drop-only when the new TTL is `None`) |
| `insert` on a new key | insert `(deadline, idx)`; entries without a TTL never enter the index |
| `remove_index` (the single release funnel: LRU eviction, lazy expiry, explicit `remove`) | drop the record **before** `release_index` resets the node and clears the deadline it is keyed by |
| `clear` | drop the index explicitly — this is the one path that bypasses `remove_index` |

`prune_expired` also carries a progress guard: a record whose node is free, or whose node's current
deadline differs, is dropped directly so the loop cannot spin. That branch degrades gracefully
rather than tripping an assertion — it runs on a data-plane path under the shard mutex, where
`skills/00-design-rules.md`'s graceful-degrade rule forbids `debug_assert!(false, ...)`. A desync
cannot corrupt results in any case: TTL correctness never depends on the index, because `get` reads
`node.expires_at` directly and the index only says where to look first.

Two coverage notes for whoever re-reviews:

- The dangerous mutation is `sync_expiration_record` indexing a rewritten entry under the **wrong**
  deadline. It is caught only by tests that drive a *rewritten* record through `prune_expired`,
  which requires a **full** cache — `test_shortened_ttl_is_pruned_at_its_new_deadline` and
  `test_extended_ttl_survives_pruning_at_its_old_deadline` exist for exactly this and were verified
  to fail under that mutation. Tests on a non-full cache never call `prune_expired` and cannot
  catch it.
- `alloc_index`'s expired-tail check is defensive, not load-bearing: `insert` drains every expired
  entry first, so it fires only for a deadline landing in the sub-microsecond window between two
  clock reads. It is untestable and correct in the conservative direction.

## Fix Suggestions Not Accepted

- Sort the LRU list by deadline, or binary-search it.
- Keep a deadline-sorted `Vec` and binary-search the insertion point.
- Add a background expiry task, timer, or timer wheel — expiry is deliberately lazy and
  request-driven; the originating task forbids a background task unless request-triggered bounded
  maintenance is measured to be insufficient, and it is not.
- Replace the index with a *lazy versioned* min-heap. (An *indexed* heap is a different and
  legitimate proposal — see the alternatives table.)
- Restore a "prune everything on every insert" pass, or prune when a free slot already exists.
- Add a third shared cache implementation, or move this cache onto TinyUFO.
- Add a metric or a WARN for index desync — the failure is benign, operator-unactionable, and this
  path forbids `tracing::` calls; the observability-restraint rule cuts against it.

## Re-evaluation Triggers

Re-open this decision only if:

- A consumer needs deadline-ordered *iteration* beyond "earliest first", which a `BTreeSet` supports
  and a heap does not — this would strengthen, not weaken, the current choice.
- Allocation under the shard mutex becomes a measured problem, in which case the indexed heap in the
  alternatives table is the designed answer and should be reconsidered on its merits.
- Profiling shows `BTreeSet` insert/remove dominating a TTL-rewrite-heavy workload; compare against
  the `update_heavy` baseline recorded above.
- The cache gains byte-weighted admission or another eviction policy, which changes what
  `alloc_index` must reclaim.

## Reference Cases

- `tasks/todo/bound-lru-expiration-maintenance.md` (P1 task, closed 2026-09-03).
- `edgion-algo/src/cache/lru.rs`, `edgion-algo/src/cache/sharded_lru.rs`.
- `edgion-gateway/benches/lru_expiration.rs` — capacity-scaled insertion benchmark
  (1K / 16K / 256K x live / mixed-expiration / update-heavy), CSV on stdout. Percentiles are over
  individual inserts; `expiration_index_payload_bytes` is payload, not resident size.
- `AGENTS.md` / `CLAUDE.md`, section "In-process cache selection" — the rule assigning explicit
  invalidation and strict replacement to `ShardedLruCache`.
- `skills/00-design-rules.md` line 163 — the graceful-degrade rule that forbids
  `debug_assert!(false, ...)` here.
- `skills/04-review/cpu-memory/regression-guards/oidc-access-token-cache-ttl-only.md` — the cache TTL is the
  single expiry authority for the only production consumer.

## Resolution references

- `task-bound-lru-expiration-maintenance` — 2026-09-03.
