# L4 listener projection storage

Status: deferred for user decision. No runtime defect or replacement is selected.
Source baseline: 52f0075e0.

## Evidence and ownership boundaries

The shared routes/l4_rebuild.rs helpers accept get_listeners callbacks returning
Vec<ListenerKey>. TCP and UDP routes_mgr wrappers copy already available listener
slices with to_vec for both full and affected rebuilds. TLS differs: its
resolved_listeners_for_route projects listener keys from attachment records, then
sorts and deduplicates an owned vector. These are configuration/rebuild costs.

Full bucketing consumes keys into owned listener buckets. Affected bucketing only
uses keys to look up already-owned buckets. The separate orphan-reference scan
collects an owned set over all cached routes. These consumers should not receive
one blanket allocation/retention assessment.

A Cow slice callback could remove TCP/UDP temporary vectors, but always iterating
with cloned keys can add TLS copies in full bucketing and orphan collection, where
its owned vector currently transfers keys. A borrowed-only callback cannot directly
serve the TLS projection. A visitor or owned/borrowed iterator adds abstraction;
a prepared TLS projection adds retained unit storage and publication work.

The sibling apply_l4_partial_update has a different boundary: init_and_insert
moves the new unit into the cache before returning owned listener keys. Keep that
helper outside an initial rebuild-only change rather than implicitly extending
borrows across cache mutations.

## Decision and acceptance

Keep the current interface, or authorize a bounded investigation comparing
protocol-specific ownership behind the existing shared orchestration. No interface
is selected. Do not duplicate the TCP/TLS/UDP rebuild algorithms or fold HTTP/gRPC
into the L4 helper. Keeping the current representation is valid if benefits do not
justify complexity or retained memory.

Before implementation, use IMPL with an exact allow-list and review:

- Allocation count/bytes, key copies, peak scratch and retained snapshot memory,
  and timing for full and affected rebuilds. Include empty/small routes, many
  routes per listener, duplicate-heavy references and TLS attachment fan-out.
  Assess full bucketing, affected lookup and orphan scanning independently.
- Exact membership, TLS sorting/deduplication and all cached listener references,
  including those ineligible for current routing. Preserve route ordering, unit
  Arc identity and destination bucket ownership.
- PGIS precreation, stale sweep before active clear-pass, the separate orphan
  sweep, and empty snapshots published to removed managers before their local
  Arc is dropped. Retained proxies must not keep forwarding deleted routes.
- Existing rebuild-lock and DashMap guard lifetimes, with no extra await or
  new lock scope. Check actual iterator behavior rather than assuming an owned
  vector necessarily releases every shard guard earlier.
- Existing successful/final-survivor plugin inputs and publication after rebuild
  lock release. Do not claim cross-listener or route/plugin-store atomicity.

Existing regressions cover active clearing, stale/orphan cleanup with retained
handles, cached-unit identity and TLS attachments. They are acceptance starting
points, not fresh performance evidence.

Relevant originals: skills/04-review/routing/regression-guards/l4-rebuild-helper-clear-pass.md
and skills/04-review/routing/intentional-design/route-architecture-consistency.md.
This record includes no production/test change, prototype, benchmark or live run.
