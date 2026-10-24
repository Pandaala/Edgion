# Evaluate canonical LRU retired-value destruction

Status: deferred for user decision. Priority: P3. Source baseline: 958826dfc.
Documentation only; no runtime or dependency change.

## Evidence and decision

Gateway `response_cache::storage::Store` stores `Arc<Entry>` in the canonical
`ShardedLruCache`. Its local shard implementation in
`edgion-algo/src/cache/lru.rs` destroys replaced values during `Inner::insert`.
Expired lookup, full-cache expiry pruning and forced eviction discard the value
returned by `remove_index` while the local mutex remains held. `Inner::clear`
resets nodes under that mutex. These paths can run a retired value's destructor
inside the shard critical section.

Explicit `LocalLruCache::remove` instead returns its value beyond the local
mutex; its caller determines the remaining lifetime. The zero-TTL insert path
calls that removal and discards the result after the removal call. Key destruction
is a separate question: `remove_index` removes the map key and takes the node
key under the lock, and clear destroys map/node keys there too. A value-only
change would not move all generic destruction outside the critical section.

For ResponseCache, dropping an Arc may only decrement a reference count. Only
the last owner releases the Entry arena, metadata and admission lease. Readers
and retained body slices can keep that owner alive after eviction. Furthermore,
Store's publication and invalidation paths (`publish_pending`, `advance_epoch`,
`remove_if_current`) hold an outer `fence` around cache operations. Releasing a
shard mutex does not prove destruction happens outside every lock.

Moving retired destruction outside the shard mutex could shorten its hold time
when a destructor actually performs substantial work. This is not a measured
bottleneck or a behavior-free edit: generic K/V may implement custom Drop,
resource release can interleave differently with other cache operations, and
ResponseCache lease release affects concurrent budget admission. Keep the current
implementation as a valid option. No retirement API or implementation is selected.

## Required boundaries

Decide value-only versus key-and-value retirement explicitly. Analyze replacement,
single eviction and multi-entry prune/clear separately. Bulk retirement requires
bounded storage and may add request-path allocation or retained memory; do not
introduce a background retirement task, shared retirement queue/pool or plugin-local
cache as an incidental optimization. Account for any new representation in the
ResponseCache infrastructure and admission budget.

Keep map, node pool, free list, recency and expiration-index mutations atomic
under the local mutex. Preserve strict replacement/removal, lazy TTL behavior,
returned value ownership and the accepted bounded BTreeSet expiration index.
Preserve existing counter meanings and ordering: insert/get increment atomics
after unlocking, clear resets them after unlocking, and statistics snapshots
remain approximate. Do not claim counters currently belong to the critical section.

Determine destructor panic and reentrancy behavior before selecting a design;
do not silently change failure policy or promise reentrant operations are safe.
Trace outer locks and lease-release/admission order in actual consumers. This
investigation does not reopen the separate [hashing decision](lru-01-shard-and-map-hashing.md),
weaken response-cache memory limits, add request tracing or add new metrics.

## Acceptance after user selection

Inventory actual canonical-cache consumers rather than assuming every V is an
Arc<Entry>. Measure lock hold time, contention, allocation and retained memory
for cheap values and representative final-owner destruction. Separate final-owner
and retained-reader cases; no throughput benefit follows solely from moving Drop.

Cover replacement, expired lookup, forced eviction, multi-expired pruning,
zero-TTL insertion, explicit removal and clear. Verify TTL/recency/index invariants,
existing counter semantics, retained-reader survival and lease-release/admission
timing. Use deterministic synchronization for concurrent destructor tests rather
than sleeps or network fixtures. Test generic custom Drop and the explicitly
selected panic/reentrancy contract. Any implementation requires a separate reviewed
plan and final independent review after the user's decision.

Original contracts: [root cache-selection rule](../../AGENTS.md),
[bounded expiration index](../../skills/04-review/cpu-memory/regression-guards/lru-bounded-expiration-index.md)
and [response-cache ownership](../../skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md).

Fresh unchanged-code verification passed 47 Algo cache, 22 Gateway cache storage
and six cache policy tests (75 total), plus workspace all-target check and Clippy
with existing warnings. These establish existing behavior, not a proposed
retirement design or destructor-concurrency contract. No prototype, benchmark,
full suite or live origin test ran.
