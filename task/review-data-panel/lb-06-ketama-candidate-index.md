# Ketama candidate-index projection

Status: deferred for user decision. No runtime or dependency change.
Inspected baseline: `bf61ceecb`; Pingora fork revision
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence

ConsistentHashRing::select starts with Continuum::node_idx(hash_key), then calls
Continuum::get_addr for each visited virtual node. It resolves that borrowed
address through addr_to_idx before SeenSet::insert. Duplicate virtual nodes
therefore repeat address hashing and lookup even when their backend was already
seen and its health predicate will not run again. This lookup itself is not an
allocation claim. The ring, address map and backend storage are built cold.

The lockfile-pinned Continuum hashes the request key once with crc32fast and
binary-searches the ring. get_addr resolves a ring point to an address-table
index, advances the cursor with wraparound, and borrows the address. It does not
rehash the request key or rebuild the ring on every step. Public node_idx returns
a ring position, not a bucket ID; private VersionedRing::get supplies the latter.

## Decision requested

Keep the current simple address API, or investigate a generation-owned index
projection. A reviewed fork API could expose a stable bucket index while a cold
mapping translates it to Edgion's retained original backend index. That mapping
scales with retained Inet entries, whereas a separately materialized vnode
projection scales with ring points. The latter can be much larger for weighted
rings. Both require construction, retained-generation memory and ownership review;
neither is selected, and this task authorizes no fork checkout modification.
Do not duplicate the ketama algorithm or add unsafe layout access to bypass its
private representation. Any dependency change must follow the repository's single
fork-family source/lockfile policy and separately approved exact scope.

prepare_ring filters non-Inet entries, takes at most MAX_CH_BACKENDS retained
Inet entries, and maps each address to its last retained original index.
Duplicate addresses and non-Inet gaps mean a bucket ID is not directly a backend
index. The full backend vector and original indices are retained. A projected
result must preserve these rules, not silently use the first duplicate or compact
position. Keep exact point ordering, collision sorting/deduplication and wraparound
from the existing dependency. No key hash or endpoint assignment change is allowed.

## Acceptance before implementation

Use IMPL with a frozen allow-list and independent reviews. Differentially compare
winner and ordered health-predicate traces for healthy primary, weighted repeated
nodes, early/late fallback, all rejection, empty/non-Inet-only inputs, duplicate
addresses, original-index gaps and retained-entry cap boundaries. Cover zero/one
and larger budgets, exact exhaustion and wraparound. Charge distinct backends,
not raw vnode steps; retain the all-distinct-rejected termination rule. Preserve
borrowed intermediate winners and final live Arc ownership from the same generation.

Do not cache live eligibility, alter rejection metrics, add a shared scratch pool,
or introduce request logs/metrics. Empty-key CH still uses the existing RR
fallback. Nonempty CH still passes through the canonical compiled resolver, with
no second health check after winning. Constructor callers and dependency versions
must be enumerated before changing an API; existing tests are not proof that a
new mapping preserves every duplicate/collision case.

Compare whole selection CPU, allocation calls/requested bytes, cold construction,
metadata size and overlap of retained old generations for small/large and weighted
rings. Separate membership work, address projection, compiled runtime lookup and
health work. No profile establishes this lookup as a bottleneck; removing one map
operation alone is not measured latency, throughput or physical-memory evidence.

## Distinct decisions and baseline verification

- [lb-05](lb-05-consistent-hash-visited-storage.md) concerns visited membership
  storage after address resolution, not the earlier per-vnode projection.
- [backend-01](backend-01-selected-slot-lookup.md) concerns compiled successful
  winner resolution and the later section slot scan.
- [backend-02](backend-02-runtime-eligibility-lookup.md) concerns live runtime-state
  lookup, not immutable ring-to-backend projection.

Fresh unchanged-runtime verification passed 41 selection tests and eight compiled
selector tests, plus workspace all-target check/Clippy with existing warnings.
No projection prototype, dependency edit, benchmark, full suite or live transport
ran. Existing tests validate the baseline, not an unimplemented representation.

Sources: Gateway lb/selection/consistent_hash.rs and lb/compiled_selector.rs;
pinned pingora-ketama/src/lib.rs (Continuum, VersionedRing, ring construction);
skills/04-review/lb/regression-guards/consistent-hash-duplicate-node-budget.md,
consistent-hash-ring-seenset-no-heap.md and rr-ch-core-shared-resolve-helper.md.
