# RegionRoute prepared writeback log targets

Status: deferred for user decision. No runtime change.
Source baseline: `5b72b1d1e`.

## Evidence and scope

In `edgion-gateway/src/plugins/http/region_route/plugin.rs`, exec_hash_range
constructs wb_targets after computing a hash when hash_calc.write_back is
nonempty. Each iteration calls session.key_set with an owned decimal hash,
ignores its result, then appends the target's diagnostic name to a comma-separated
String. `edgion-resources/src/common/key_accessor.rs::KeySet::as_log_str`
constructs another String for each target (hdr, res_hdr, cookie or ctx prefix).
The joined target text depends only on the immutable base plugin configuration.
The optional overlay replaces regions, not this writeback list.

Precomputed-hash success returns before this work. Missing calculation/input and
empty writeback lists also avoid it. Computed hashes still write back when no
region range matches; target logging precedes that lookup. Target names describe
attempted writes, not successful writes.

## Decision

Retain current formatting, or prepare one joined raw diagnostic string in the
plugin generation. Preparation removes repeated target-name formatting but adds
construction work and retained storage, including unused configurations and old
in-flight generations. Measure target count/name length and computed-hash traffic
before selecting a representation. Do not assume a small fixed total bound from
the response-header name validator or introduce a new list limit.

The dynamic hash value and each setter's owned value are separate costs. Preparing
labels does not remove those strings, log storage or the hash computation. No
allocation count, throughput improvement or whole-entry allocation-free claim has
been measured. This differs from [region-01](region-01-prepared-lookup-indexes.md)
and [region-02](region-02-prepared-endpoint-projection.md), which concern selection
indexes and endpoint parsing respectively.

## Acceptance before implementation

- Preserve list order, duplicates, exact prefixes/separators and attempted-write
  diagnostics even when setters fail. Keep logging after all writes and preserve
  missing-input, passthrough, empty-list and unmatched-range behavior.
- Prepare raw text only. Apply the existing sensitive formatter at request time;
  never bake startup masking into configuration. Add no tracing, metric or new
  request-level observability mechanism.
- Keep data attached to its immutable plugin generation, cover both bound and
  inline construction and reload, and retain old-generation correctness. No
  serialized/schema-visible derived field, global cache or request-time lazy
  preparation. Reuse the canonical KeySet diagnostic representation.
- Compare empty/one/many targets, all four variants, duplicates, failed setters,
  precomputed/computed hash and unmatched ranges. Verify exact log behavior in
  supported sensitive modes using the project's isolation rules. Measure cold
  work and retained bytes separately from request allocations and CPU.
- Require independent IMPL plan/final review with an exact file allowlist and
  repository checks before any implementation.

Fresh baseline verification: 23 Gateway RegionRoute-selected tests passed. This
is existing-behavior evidence, not validation of a prepared representation.
The [effective table contract](../../skills/04-review/routing/regression-guards/region-route-effective-table-validation.md)
continues to govern overlay validation, fail-to-base and one-hop failover.
