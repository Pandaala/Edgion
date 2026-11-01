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
the request or connection. Consumers capture once per decision when their cold demand requires ConfigData. HTTP entry conditions, nested ExtensionRef and ACL membership share the dispatch-local view. Stream GCIR selection and selected ACL membership share one view; consumers never collect matcher vectors or copy CIDRs.

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
- Region-route setting consumers borrow the local identity and table from the live raw resource.
  `TypedIndexes.region_tables` stores only a compact `RegionTableValidation`
  proof for each standalone-valid table, not a cloned `ClusterRegionSetting`.
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
- Selector changes need no derived rebuild. ClusterRegionSetting table changes
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
- Active reference-only HTTP, connection and TLS ACL decisions use one
  dispatch- or connection-local snapshot. Disabled or policy-masked entries are
  not constructed and introduce no executable lookup demand. GCIR reads its
  optional Selector and selected profile's IpList keys through the same view;
  every structurally valid executable profile contains references.
- HTTP, connection, TLS-route and global-connection IP restriction borrow ordered
  authorized matchers through `EdgionConfigDataSnapshot::ip_list_matchers`.
  Source order is the authored reference order; consumers have no inline
  matchers. No temporary `Vec` or `SmallVec` projects the union.
  The canonical evaluator stops after a decisive match and preserves group Arc
  identity. A matched-group result still clones an existing `Arc<str>` and performs
  reference-count work; it does not allocate a new group value. Removing these
  projections does not establish that the complete consumer path is allocation-free.
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

Cold preparation has useful work to retain: `PreparedSlot::new` caches each target
identity result, and `BoundIpListRefs::side` pairs prepared slots with actual fields
through a linear zip. Twenty IP sources in one list require twenty slot visits,
not twenty searches of that list. The Gateway then compiles canonical keys for
runtime use. Controller preparation and Gateway cold compilation are distinct
steps; this does not claim that the Controller serializes `PreparedSlot` objects.
On 2026-10-12, the user chose to retain the current preparation/binding mechanism
and close the proposed simplification follow-up without production changes.
The bounded source analysis established no measured benefit sufficient to justify
a binding/lifetime refactor. This is not whole-contract acceptance or a prohibition
on future evidence-backed optimization of cold traversal.

ARCH-071: Standalone and condition IpList topology comparisons borrow the stored
ConfigData name and optional namespace against the actual owner fields. Snapshot
preparation still owns its strings; only temporary comparison copies are removed.
Omitted and explicitly empty namespaces remain distinct, other wire variants
reject, and order, side, presence and owner gates remain unchanged. Controller
material and denial updates remain outside authored topology. This does not
replace the preparation/binding mechanism or claim allocation-free traversal.

The Gateway retains only canonical target keys in the compiled plugin. It continues
to capture one immutable ConfigData snapshot per decision, preserving atomic IpList
updates and live content refresh for pinned plugin generations. Selector and other
ConfigData target roles are not migrated by this change.

### Bound remaining plugin fields

RAP/Canary/GCIR Selector and RegionRoute keys are built from actual EP/ESP field loans before cache reuse. Invalid target slots rebuild each consumer's established denial/base outcome; invalid owners cannot reuse a prior external executable. RegionRoute specifically executes its failOpen/statusOnError policy for an unusable target; it has no base-table fallback. Fresh valid retarget generations retain the existing fresh-cache contract. GCIR still captures Selector and IpLists together, and Region raw value/proof publication remains paired. RAP URL documents are expanded in Controller and introduce no live Gateway document lookup. Unit coverage proves bounded constructor/cache/generation behavior, not live cross-process requeue traffic.

### ARCH-028 B2 condition consumers

Shared IP conditions and RAP primary membership now borrow central IpList matchers.
One dispatch-local adapter snapshot covers required-root preflight, all later
condition reads and RAP Selector profile-index selection, including nested
ExtensionRef loops. Current raw visibility and typed index membership are checked
in that same handle. Separate callbacks may see newer generations. Ordinary
missing-list recovery does not require an EdgionPlugins generation change. The
B1 HTTP ACL wrapper still had a separate inner snapshot at the historical B2
checkpoint. B5 replaces that boundary with the same dispatch-local scalar ACL
capability used after required-root preflight and condition reads. This is one
callback/dispatch view, not a whole-request or cross-store transaction claim.
