---
name: endpoint-slice-ownership-transitions
description: Use when reviewing same-key EndpointSlice relabels, compressed delete-to-recreate batches, affected-Service rebuilding, or retired backend runtime state.
knowledge_type: regression-guards
---

# EndpointSlice ownership transitions rebuild both Service generations

## Conclusion and scope

`ServiceDiscoveryCoordinator::update_endpoint_slices` treats every incoming EndpointSlice as an
upsert against the single `CoordinatorState::endpoint_slices` source map. If the same resource key
moves from one `kubernetes.io/service-name` owner to another, both the owner from the stored record
and the owner from the incoming record are affected. The transaction rebuilds both Service
projections from the resulting source state and publishes one joint generation.

This rule covers ordinary updates and conf-sync batches where remove overlaps add or update for the
same key. A replacement is visible only through its new Service. Retired endpoint runtime state is
detached and removed or drained; a late completion carrying the old membership and endpoint
identity is rejected.

## Rationale and evidence

The former `EpSliceStore` forward/reverse indexes could diverge when an `add` overwrote an existing
key. Production mutation now belongs only to `ServiceDiscoveryCoordinator`; `EpSliceStore` is a
read-only compatibility facade over the published generation.

In `update_endpoint_slices`, add and update inputs are normalized into one incoming map. Before an
incoming record replaces the stored record, the coordinator inserts the stored record's Service key
into the affected set, then inserts the incoming Service key. A remove that overlaps incoming data
does not delete the replacement, but marks it fresh so a compressed delete-to-recreate receives a
new source identity. `transact` builds each affected projection by filtering the complete staged
source map, publishes the generation, and calls `activate_service` for changed Services.

Activation first makes the runtime cell inactive, then freezes every old endpoint state, revokes
its outlier identity, and either drains or removes its backend. If the old Service source no longer
exists and owns no remaining slices, its projection and runtime cell are removed.

## Alternatives and limits

Do not restore a separately mutable Service-to-EndpointSlice reverse index or derive cleanup from
raw add/update/remove counts. The resulting source state and the union of old and new owners are the
ownership contract.

A Service resource that still exists without EndpointSlices may retain an empty Service runtime
cell. That is a valid empty projection, not stale endpoint ownership; it contains no active endpoint
entries. This guard does not define Kubernetes event classification or cross-kind ordering outside
the discovery coordinator.

## Verification and re-evaluation

The coordinator unit tests cover:

- a same-key ordinary update from Service `old` to Service `new`, including removal from the old raw
  projection, retirement of the old runtime entry, rejection of a late health completion, and sole
  publication under the new Service;
- a remove-plus-add compressed recreate for the same key after the old Service source disappears,
  including removal of its unavailable projection and runtime cell.

Verified on 2026-09-20 with:

```text
cargo test -p edgion-gateway --lib backends::discovery::coordinator::tests::endpoint_slice_
```

Both targeted tests passed. Re-evaluate this guard if EndpointSlice mutation leaves
`ServiceDiscoveryCoordinator`, a reverse ownership index is reintroduced, affected-Service
calculation changes, or runtime activation is separated from joint generation publication.

## Resolution references

- `endpoint-slice-ownership-transitions` — fixed and regression-covered on 2026-09-20; source:
  `edgion-gateway/src/backends/discovery/coordinator.rs`.
