# Evaluate canonical LRU routing and map hashing

Status: deferred for user decision. Priority: P3. Source baseline: ca0561344.
No runtime or dependency change.

## Evidence and decision

Gateway response_cache::storage::Store::lookup uses the canonical
ShardedLruCache with a 32-byte digest key and Arc<Entry> value. When more than
one shard exists, shard_index hashes the borrowed query with DefaultHasher
before entering a LocalLruCache. The local HashMap uses its own default
RandomState hashing under the shard mutex. One shard already bypasses routing
hashing. This is routing hashing followed by map hashing, not a universal
exactly-two-hashes count: insertion and removal/expiry paths can hash again.

The two hash operations have different owners and purposes. A routing hash is
not automatically a valid precomputed hash for a particular map's seed. Nor is
a digest-key response cache representative of every shared-cache consumer.
Existing borrowed-key lookup already avoids constructing a temporary owned key.
On a hit, the local cache clones V under its mutex and updates recency before
returning; ResponseCache's Arc clone does not duplicate the entry payload.

Possible investigations are keeping the current implementation, preparing a
routing hash with a bounded key owner, or introducing a canonical prehashed
lookup interface that explicitly binds hash provenance to map ownership. The
last two add representation/API complexity and possibly retained metadata.
No custom hasher, map dependency or prehash representation is selected. Measure
the actual short-key workload before treating the second hash as significant.

## Required boundaries

Preserve Hash/Eq coherence between stored K and borrowed Q, including unsized
borrowed keys. Full key equality must resolve collisions; a short hash must not
replace the key or become an authorization/identity proof. Review adversarial
key distribution, hash seeds and reuse across cache instances/generations.

Routing affects which keys compete for each shard's fixed quotient/remainder
capacity and LRU order. Changing it can change eviction behavior even with the
same total capacity. Any such semantic change needs an explicit decision; do
not silently require power-of-two shard counts or substitute a faster hash.
Retain accepted non-power-of-two shard configurations and the single-shard path.

Keep strict replacement/removal, TTL semantics, lazy expiry, bounded deadline
index, per-shard locking, operation-counter meanings and existing torn aggregate
snapshots. Retain independent returned value ownership without references
escaping a released mutex. Do not add a plugin-local map, third shared cache,
background expiry task or query-time diagnostics. The accepted BTreeSet expiry
design is not reopened by a hashing investigation.

## Acceptance after user selection

Compare single and multiple shards, including non-power-of-two counts, live hits,
misses, updates, expired hits, removal and churn. Include 32-byte ResponseCache
keys and representative longer keys from actual other consumers; distinguish
canonical helper cost from whole-request work and cache-bypass traffic.

Measure CPU, lock hold time, contention, hash operations, allocations and retained
metadata across instances and old generations. Account for new key/hash fields
in ResponseCache's conservative infrastructure/admission budget, not just RSS.
Test forced collisions, borrowed queries, seed/generation isolation, distribution
and per-shard eviction behavior. Preserve TTL/statistics and value-survival tests.
No throughput benefit follows merely from deleting a hash call.

Original contracts: [cache unification](../../skills/01-architecture/00-common/05-unification-points.md),
[bounded expiration index](../../skills/04-review/cpu-memory/regression-guards/lru-bounded-expiration-index.md)
and [response-cache ownership](../../skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md).

Fresh unchanged-code verification passed 47 Algo cache, 21 Gateway cache storage
and six cache policy tests. These establish existing behavior, not a proposed
hash representation. No prototype, performance benchmark, full suite or live
origin test ran.
