---
name: route-hostname-matching
description: Shared immutable Route hostname radix, persistent contents, candidate ordering, and update granularity.
status: shipped
---

# Immutable hostname matching — Routes

## What we run today

`edgion-algo/src/matcher/radix_hostname/mod.rs` owns the generic
`RadixHostnameMatcher<T>` used by HTTPRoute, GRPCRoute and TLSRoute. Its frozen
reverse-byte compressed radix maps exact and wildcard terminals to private,
generation-local IDs. A persistent `rpds::HashTrieMapSync` maps those IDs to
opaque `Arc<T>` contents. The tree knows nothing about Routes, Listeners,
backend selection, errors, catch-all policy, or publication.

Single-pattern grammar and input normalization live beside the index in
`edgion-algo/src/matcher/radix_hostname/`, allowing Controller, Resources, and
Gateway to share them without a business-crate dependency. Resources retains
Route hostname-list cardinality and required/optional policy. The matcher
accepts only a checked, normalized borrowed `RadixHostnameQuery`. Configured names
are lowercase DNS names, including short names; bare `*`, IP literals, partial
wildcards and malformed lists are rejected. HTTPRoute/GRPCRoute permit 0..16
names; TLSRoute requires 1..16. Invalid entries reject the entire Route list.

The immutable topology uses exactly sized flat node, packed child-entry and
edge-byte pools. Edges are stored in query-forward order, so traversal compares
one continuous borrowed query slice without constructing a reversed request.
After descent a lazy iterator returns exact first, then follows parent indices
to yield wildcard terminals from longest to shortest suffix. Thus exact wins,
then longest wildcard suffix, then shorter matching suffixes. `*.example.com`
matches `a.example.com` and `a.b.example.com`, never `example.com` or
`notexample.com`. Exact and wildcard terminals at the same suffix are distinct.
HTTP/gRPC adapters own their separate hostless groups; TLSRoute has none.

## Contract

For query length L and K matches, the radix walks at most L bytes and performs
binary child searches over a DNS alphabet bounded independently of hostname
count H. Content lookup uses the persistent map for each returned ID; its cost
is not part of the radix's byte-walk bound. Queries allocate no heap memory,
clone no Arcs, and acquire no lock. A 253-byte checked name has at most 127
nonempty labels: at most 126 wildcard ancestors plus one exact terminal. The
iterator is lazy and stores no fixed candidate array; it never truncates matches.

Construction sorts by reversed suffix and exact-before-wildcard terminal kind,
then assigns private generation-local IDs and freezes exact-capacity pools.
The ordered pattern enumeration has no external ordering contract. Construction
is a cold operation, not an O(1) update. `with_changes` coalesces
operations by pattern and compares final occupancy: content-only changes reuse
the entire tree and path-copy changed persistent-map branches. Add/remove with
no net occupancy change does not rebuild it. A changed key set rebuilds the
tree and remaps contents by canonical pattern, never by an old numeric ID.
Unchanged payload Arcs survive either path. `with_groups` discovers changes
by enumerating all groups, so its O(H) discovery remains explicit even when it
reuses the tree. Equal cardinality plus complete old-pattern membership proves
key-set identity; replacements then use the old pattern order's IDs directly,
without converting a full list into a second delta/coalescing collection.

Flat construction is fallible because node, child, edge and terminal metadata
use checked `u32`/`u8` representations. HTTP, gRPC and TLS owners map a capacity
error to a complete empty physical-Listener table and publish it through their
existing outer snapshot: no old table, partial table, wildcard or catch-all
fallback survives the failure.

The protocol owner publishes one immutable outer snapshot containing all
dependent data; the matcher adds no nested ArcSwap. Readers holding an old
snapshot retain its matching tree/content pair. Released generations reclaim
their unshared allocations normally. There is no cross-Listener or cross-kind
atomic transaction, and no requirement that opaque T implement Clone.

Adapter granularity is intentionally different:

- HTTP deltas rebuild touched hostname path groups and remove every unit owned
  by changed/deleted Routes; untouched path groups survive. Full-list/Gateway
  rebuilds may rebuild payloads while reusing a same-key tree.
- gRPC rebuilds its rank array and all rank-dependent five-stream indexes
  together. Cached match units can be shared; the previous hostname tree can
  survive even a full-list rebuild.
- TLS regroups accepted candidates and reuses unchanged ordered candidate
  groups when unit identities and attachment indices agree. Full regrouping
  still scans units; it is not a delta-only discovery algorithm.

All adapters rank by authored Route patterns. Listener intersection is an
eligibility check, not promotion of a wildcard Route to exact priority. TLS
checks hostname and logical ownership on the same accepted attachment and
retains runner-ups. Only candidate no-match permits continuation; Listener
exclusivity and post-selection policy/backend failures do not authorize a
weaker Listener or a retry through other owners.

Determinism and boundaries are covered by the core's exhaustive small-pattern
oracle, insertion-order checks, exact/wildcard same-suffix tests, maximum-depth
checks, old-snapshot ownership tests, and actual adapter builder tests. These
are coverage descriptions, not a claim that a new verification run passed.

## Why this algorithm

One canonical implementation avoids different wildcard depth, short-name and
boundary behavior across three Route protocols. Separating topology from
contents preserves the immutable read path without rebuilding hostname edges
when only a route payload changes. Keeping policy outside the index prevents
certificate, Listener, and backend behavior from leaking into a generic match.

## Prior art comparison

| Implementation | Decision and reason |
|---|---|
| Gateway API [v1.5.0 hostname definitions](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/shared_types.go) and Route definitions | Adopt multi-label wildcard suffixes and authored hostname specificity; intersection alone does not settle runtime Listener ownership. |
| Shared RFC 6125 certificate consumers | Keep `edgion_algo::matcher::rfc6125_hostname` semantically separate. Its certificate-selection/identity contract is not Route semantics. The benchmark has an isolated multi-label hash reference; it is not a new production matcher. |
| Former Route `RadixHostMatchEngine` | Replace its payload-coupled tree and permissive configuration handling with the shared checked index. Its useful exact/wildcard/empty/payload tests are covered by the new core; old acceptance of uppercase configuration or nested wildcards is intentionally not retained. |
| [rpds 1.2.1 hash-trie implementation](https://github.com/orium/rpds/blob/v1.2.1/src/map/hash_trie_map/mod.rs) | Reuse the existing persistent map for content path-copying; do not introduce a custom persistent container or require payload cloning. |
| [ArcSwap consistent-snapshot pattern](https://docs.rs/arc-swap/latest/arc_swap/docs/patterns/index.html) | Keep one owner-level publication and retain snapshots where decisions must agree; independent inner swaps could mix a tree generation with another generation's contents/ranks. |

## Known limitations

Topology changes still rebuild the whole per-owner tree. Many independent
Listeners multiply cold build/storage cost. Held old snapshots retain memory;
there is no forced reclamation. Payload construction and whole-cache affected
Listener discovery can dominate updates even when the tree is shared. No
claim is made that every update is proportional only to changed hostnames.

Route wildcard matching does not change certificate selection or RFC 9525 DNS
identity verification. It does not establish full Gateway API conformance.

## Replacement criteria

Consider another layout only after a correctness-equivalent implementation
improves measured lookup, update and retained-generation memory on realistic
Listener/key distributions without weakening immutable ownership or moving
allocation to the request path. Preserve exact/suffix continuation and final
occupancy semantics, not just first-hit throughput.

`edgion-algo/tests/benches/radix_hostname.rs` is a dependency-free, `harness=false`
benchmark with 10/1,000/100,000 keys, exact/wildcard/miss/continuation lookups,
200/253-byte continuous edges, shared-suffix and high-fanout shapes, full
builds, content patches, topology changes, maximum-depth queries, multi-Listener
builds and held-generation reclamation. Its counting allocator
measures requested bytes, not RSS; batch means are not latency percentiles.
Report machine, profile, key shape and command with any result. It adds no
production metrics or dependencies.

The selected 20-byte node and 5-byte packed child layout was measured on an
Apple M4/16 GiB macOS aarch64 host with Rust 1.96.1 in the optimized bench
profile. Against the recursive baseline, the 100,000-key build's net live-byte
delta fell 27.8% (the matcher-only final-reader release fell 18.4%), retained
add/remove generation deltas fell 20.6%, and all common lookup medians
improved (exact/wildcard/short first-hit by 38.1%/48.3%/63.6%). Continuous
200/253-byte edge hits improved 87.4%/89.6%. The artificial full 127-candidate
walk regressed 10.9%; this is retained as a documented worst-case cost of the
smaller lazy iterator because ordinary first-hit paths and retained memory met
the predeclared acceptance gate.

## References

- `edgion-algo/src/matcher/radix_hostname/`
- `edgion-resources/src/hostname.rs`
- [Route architecture](../01-architecture/02-gateway/03-routes/SKILL.md)
- [Configuration update discipline](../01-architecture/02-gateway/11-conf-handler-guidelines.md)
- [TLS attachment regression contract](../04-review/routing/regression-guards/tls-hostnames-per-attachment.md)
- [RFC 9525 section 6.3](https://www.rfc-editor.org/rfc/rfc9525.html#section-6.3): certificate wildcard identity, not Route selection.
