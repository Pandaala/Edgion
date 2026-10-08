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

Authorization decisions that combine live references capture one owned snapshot
handle and resolve every raw Selector and IpList matcher through that handle.
This makes one decision generation-consistent even when publication races with
the request or connection. Consumers capture lazily: inline-only IP restriction
decisions, and global-connection decisions with neither a Selector nor selected
profile references, do not load the store.

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
  live data-plane consumers. When a selected profile contains live IpList
  references, both the Selector and those matchers must resolve through the same
  captured snapshot handle.
- `IpList` is a live data-plane reference. Its grouped CIDRs are compiled at
  configuration time and looked up dynamically by request and stream IP
  restriction consumers. Deny and allow keys are resolved from the same captured
  handle and preserve their configured order.
- Region-route overlay consumers borrow the table from the live raw resource.
  `TypedIndexes.region_tables` stores only a compact `RegionTableValidation`
  proof for each standalone-valid table, not a cloned `RegionRouteOverride`.
  `get_region_route_by_key` returns the raw resource and proof from one snapshot;
  the consumer then checks its modulo without repeating whole-table validation.
  The former duplicate-overlay index had no production caller and was removed;
  that historical removal does not prohibit useful configuration-time proofs.

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
- Selector changes need no derived rebuild. RegionRouteOverride table changes
  rebuild the compact validation proof at publication time. The raw table and
  proof remain atomic; invalid, disabled, or different-type generations must not
  retain a previous proof. See the
  [effective-table validation boundary](../../routing/regression-guards/region-route-effective-table-validation.md).

Serialization failure is treated conservatively as changed. Variant transitions
remove the possible IpList matcher entry before installing the new variant,
preventing stale index residue. A disabled IpList has no matcher entry;
re-enabling compiles the current generation.

## Limits

- Partial publication still performs an O(n) shallow clone of the Arc-valued outer
  maps. It avoids O(n) resource cloning and typed recompilation, but is not a
  persistent-map implementation.
- `full_set` remains a complete rebuild for reconnect and relist semantics.
- A captured `EdgionConfigDataSnapshot` is internally generation-consistent and
  keeps its generation alive while borrowed raw entries and matchers are used.
  Authorization code must not combine direct store getters or independently
  captured handles in one decision. Raw-only single-lookup consumers may keep
  using the direct getter because they cannot tear related values.
- Reference-free HTTP, stream, and TLS IP restriction decisions avoid snapshot
  capture. Global connection IP restriction captures unconditionally only when a
  Selector must be read; without a Selector it captures only when the selected
  base profile has deny or allow references.
- HTTP uses one inline-capacity `SmallVec` per authorization side to combine
  inline and referenced matchers. The three stream consumers use their
  `SmallVec`s only for resolved reference matchers, then the existing resource
  helper creates a `Vec` per side to prepend inline matchers. The HTTP `SmallVec`
  spills when its inline matcher count plus resolved references exceeds four; a
  stream `SmallVec` spills when resolved references exceed four. The stream helper
  `Vec`s may heap-allocate. A matched-group result clones an existing `Arc<str>`
  and performs reference-count work; it does not allocate a new group value. Do
  not describe these paths as allocation-free.
- Production load-clone-store publication relies on the conf-sync single-writer
  contract. The test-only insertion helper uses `ArcSwap::rcu` and rebuilds from
  the newest value on each retry so concurrent unique-key inserts are not lost.

## Verification and re-evaluation

Focused unit coverage checks equivalent-update suppression, unchanged Arc reuse,
metadata- and description-only updates, matcher input changes, enable toggles,
variant transitions, removal, add-generation replacement, full/partial effective
equivalence, warning scope, and concurrent test-helper insertion. A deterministic
generation test captures the old handle, atomically migrates a Selector plus
multiple ordered IpList keys, and proves all of the following: both complete
generations deny the probe, the deliberately torn old-Selector/new-list tuple
misses and reaches its allow default, the retained old handle still resolves the
old ordered pair, and a recaptured handle resolves the new ordered pair.

The existing
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
- `D0048` / campaign batch `B0123` — fixed multi-lookup authorization tearing by
  introducing decision-scoped snapshot capture, 2026-09-08.
- `D0065` — extends RegionRoute publication with configuration-time validation
  proofs; the historical claim that overlays need no derived work is superseded.
  Verification for this extension is tracked in the linked routing rule.

## Standalone IpList owning references

HTTP, connection, TLS-route and GCIR profile allowRefs/denyRefs use
`PreparedIpListReferences` at cold construction. The Controller shares the condition
IpList permission decision with fixed EP/ESP source and ConfigData target roles.
An omitted owner namespace resolves targets in `default`; reverse dependencies and
raw source/cache/rate identities retain the original resource key. Invalid target
slots have no read, Grant query or dependency. Authored allow intent survives a
filtered external set; deny-only failed external sets are dropped. Invalid owning
bindings reject reference-bearing construction independently of target availability.
Existing disabled-entry policy and structural config validation remain unchanged.

The Gateway retains only canonical target keys in the compiled plugin. It continues
to capture one immutable ConfigData snapshot per decision, preserving atomic IpList
updates and live content refresh for pinned plugin generations. Selector and other
ConfigData target roles are not migrated by this change.
