---
name: edgion-config-data-incremental-atomic-publication
description: Use when reviewing Gateway EdgionConfigData partial publication, typed-index reuse, or Controller-expanded versus data-plane-live references.
knowledge_type: regression-guards
---

# EdgionConfigData partial publication is incremental and atomic

## Conclusion and scope

`EdgionConfigDataStore` publishes the raw resource map and its derived typed
indexes in one immutable `Snapshot` through one `ArcSwap`. Partial updates mutate
only affected keys in cloned Arc-valued outer maps. Unchanged raw resources and
`IpRadixMatcher`s retain their `Arc` identity; an equivalent update does not
publish a new snapshot.

Add and update have different generation semantics. An update may reuse an
equivalent raw or typed value. An add always replaces and rebuilds the affected
typed value, even when the old store entry has identical content, because the
conf-sync compressor represents delete-then-create coalescing as an add while the
old generation may still be present in the handler store.

## Reference ownership boundary

Do not infer that every `EdgionConfigDataRef` needs a Gateway typed index.

- `RequestAccessUrlAllowList` data and `ProxyProtocolTrust` policy are resolved or
  expanded by the Controller into their consuming resource before publication.
  The Gateway must not build a second live-reference mechanism for them.
- `KeyList` and `Misc` have no current Gateway typed consumer.
- `Selector` is intentionally read from the raw snapshot and type-guarded by its
  live data-plane consumers.
- `IpList` is a live data-plane reference. Its grouped CIDRs are compiled at
  configuration time and looked up dynamically by request and stream IP
  restriction consumers.
- Region-route overlay consumers read and type-guard the live raw snapshot. The
  former `RegionRouteOverride` typed index and public getter had no production
  caller and were removed; cloning an unused duplicate would not improve the
  request path.

Controller reverse-reference tracking and dependent requeues are complementary:
they refresh referring resource status and resolved configuration, but do not
replace the Gateway's deliberately live Selector, IpList, or overlay lookup.

## Rationale and evidence

The canonical authority is the no-fake-incremental rule in
`skills/00-design-rules.md` section S and
`skills/01-architecture/02-gateway/11-conf-handler-guidelines.md` section 3.1.

Before the fix, `EdgionConfigDataStore::update` applied a delta to a clone of the
complete raw map and then called `build_indexes` for every entry. An unrelated
change therefore recompiled every enabled IpList, re-cloned every region-route
overlay, and re-emitted every partial-good IpList warning.

The corrected input projections are deliberately narrower than raw-resource
equality:

- Raw equality compares the complete wire-visible JSON value. Metadata, status,
  or spec changes therefore remain visible. `EdgionConfigDataSpec::unknown_fields`
  is `#[serde(skip)]` and is not part of the Gateway conf-sync wire value.
- IpList matcher equality compares `spec.enable` plus the ordered group names and
  ordered CIDR strings. `IpGroup.description`, resource metadata, `active`, and
  `visibility` do not affect `build_grouped_matcher_partial` and do not trigger a
  recompile or repeated warning.
- RegionRouteOverride and Selector changes need no derived rebuild; publishing the
  changed raw resource is sufficient for their live consumers.

Serialization failure is treated conservatively as changed. Variant transitions
remove the possible IpList matcher entry before installing the new variant,
preventing stale index residue. A disabled IpList has no matcher entry;
re-enabling compiles the current generation.

## Limits

- Partial publication still performs an O(n) shallow clone of the Arc-valued outer
  maps. It avoids O(n) resource cloning and typed recompilation, but is not a
  persistent-map implementation.
- `full_set` remains a complete rebuild for reconnect and relist semantics.
- A single snapshot load is internally generation-consistent. Separate getter
  calls can straddle a concurrent publication and do not provide request-wide
  consistency.
- Production load-clone-store publication relies on the conf-sync single-writer
  contract. The test-only insertion helper uses `ArcSwap::rcu` and rebuilds from
  the newest value on each retry so concurrent unique-key inserts are not lost.

## Verification and re-evaluation

Focused unit coverage checks equivalent-update suppression, unchanged Arc reuse,
metadata- and description-only updates, matcher input changes, enable toggles,
variant transitions, removal, add-generation replacement, full/partial effective
equivalence, warning scope, and concurrent test-helper insertion. The existing
`CanaryHotSwap` dynamic scenario provides related cross-process coverage by
updating a Selector EdgionConfigData and checking the resulting data-plane route.
Its update fixture also reapplies the referring EdgionPlugins resource, so it is
not isolated proof that an EdgionConfigData-only update reached this store. No
integration scenario was rerun as part of this correction.

Re-evaluate this rule if the conf-sync compressor changes add/update
classification, a second production writer is introduced, a new ConfigEntry
variant gains a live Gateway consumer, or an existing Controller-expanded
reference becomes a data-plane-live reference.

## Resolution references

- `incremental-publication-01-edgion-config-data` — fixed from pending task,
  2026-09-07.
