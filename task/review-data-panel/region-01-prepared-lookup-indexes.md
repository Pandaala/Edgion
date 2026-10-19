# RegionRoute prepared lookup indexes

Status: deferred optimization for user decision. No runtime change.
Source baseline: 0fd6f42a0.

## Evidence

HTTP request plugin dispatch reaches RegionRoute::run_request in
edgion-gateway/src/plugins/http/region_route/plugin.rs. Direct-region and
configuration-map rules resolve the selected region with lookup_by_name, an
ordered linear scan. Hash-range rules use lookup_by_hash, another linear scan.
A selected region with failover_to performs one additional name scan, then routes
to that target without following its own failover link.

The shared RegionTableValidation::new in
edgion-resources/src/resources/edgion_config_data/region_route.rs validates unique
names, disjoint inclusive ranges, endpoint syntax and the failover graph. It
builds temporary name/range/target structures, sorts the temporary range vector,
and retains only max_hash in its compact proof. It does not sort the resource's
region array. The table_bounds_are_inclusive_disjoint_and_consumer_specific test
explicitly accepts the reversed [gap, first] array. Binary search over the existing
raw array would therefore be incorrect.

Base tables belong to immutable plugin configuration. Live overlay tables belong
to raw EdgionConfigData resources; their compact proof is published alongside the
raw map in one ArcSwap Snapshot. get_region_route_by_key reads both from that same
snapshot. EffectiveRegions retains the raw resource Arc while borrowing its table.
No per-request table clone or repeated full-table validation needs removal here.

## Decision and alternatives

Decide whether to retain linear scans or evaluate prepared indexes in a new IMPL
plan. There is no workload measurement establishing the useful crossover size.

- Keep linear scans for small tables: no additional retained index, construction
  cost or lookup indirection. Early hits may examine only one region.
- Build a sorted interval index containing original row positions, then search
  that index and borrow the original row. Preserve gaps and inclusive bounds;
  a predecessor candidate still needs its upper-bound check. Do not reorder the
  raw resource or assume that authored order is sorted.
- Build a name-to-row map or a sorted name-index vector. An owning string-key map
  duplicates names; sorted row positions can compare names through the owning
  table, at the cost of indirection and string comparisons. Avoid self-referential
  borrowed keys or unsafe lifetime extensions.
- Optionally prepare one-hop failover row positions with the table. A chain
  a -> b -> c must still select b when starting at a; precomputation must not turn
  validation's graph walk into transitive request routing.

Indexes add retained data per live configuration generation and work on publication.
An inline index value copied with every outer snapshot map could also increase
unrelated update cost; immutable shared index ownership is an option to assess,
not a reason to add a second store. Base tables and globally shared overlays have
separate construction owners but need one consistent index-building mechanism.
Small-table thresholds and retained representations need evidence rather than an
arbitrary cutoff. No prototype or benchmark was run in this audit.

## Acceptance for a selected implementation

- Preserve source order/first-nonempty extraction, direct and configuration-map
  fallback, bypass keys, ordered first-match rules and terminal hash-rule behavior.
  Keep hash algorithms, modulo, passthrough validation and writeback unchanged.
- Preserve exact original rows and names, inclusive endpoints, gaps, arbitrary
  authored order, unknown-name fallthrough and one-hop failover. Do not accept
  invalid tables or move validation to the request path.
- Retain fail-to-base for missing, disabled, wrong-type, invalid or modulo-incompatible
  overlays; absent overlay regions inherit the base. Preserve local-region pass,
  strict selected-endpoint parsing, upstream address safety and jump outcomes.
- Publish each overlay index with its exact raw table and validation proof. A
  request must retain the necessary generation owners through selection; reading
  an index from a second snapshot is not acceptable. Do not duplicate whole raw
  tables or introduce a mutable request-side lookup cache.
- Build base indexes at plugin construction and overlay indexes at the canonical
  ConfigData publication boundary. Preserve equivalent-update no-publication,
  unrelated-index reuse, add/delete-create replacement and stale-index removal
  for disabled/invalid/type-changed resources. Avoid rebuilding every index for
  a partial update.
- Differentially compare prepared and linear lookups over unsorted tables, gaps,
  inclusive boundaries, unknown names, every failover shape and overlay rotations.
  Verify old retained generations remain valid after replacement or deletion.
- Measure lookup latency, construction work, allocations and total retained memory
  for small and large tables, early/late/missing matches, varying name lengths,
  multiple plugin consumers, updates and overlapping live generations. Include
  the cost of index-map copying or Arc ownership, not only isolated lookup time.
- Keep existing access-log text/masking, outcomes and metrics. No new observability,
  resource fields, CRD changes or broadened cross-crate cleanup are authorized.
  Freeze an exact file allow-list in the implementation plan after selection.

## Original knowledge and verification

The current contracts remain
skills/04-review/routing/regression-guards/region-route-effective-table-validation.md
and
skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md.
Their compact-proof design avoids duplicate tables; it does not by itself prove
that every possible prepared index is beneficial or forbidden.

Fresh serial verification of unchanged code passed 61 network-free tests:
23 Gateway RegionRoute, 22 ConfigData and 16 resource-layer region tests.
Formatting, workspace check/Clippy and agent-document/diff validation passed.
No full workspace test suite, live transport or performance benchmark ran.
