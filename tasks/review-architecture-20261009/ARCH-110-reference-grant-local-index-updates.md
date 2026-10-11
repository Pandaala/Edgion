---
id: ARCH-110
status: candidate
kind: architecture-debt
priority: P3
partition: PART-421
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Maintain the ReferenceGrant namespace index from the changed grant

## Current owner and concrete change scenario

ReferenceGrantStore keeps its raw map and target-namespace index under one State lock. This is a useful atomic ownership boundary: readers cannot observe a raw mutation with an older index. However, upsert and remove invoke rebuild_namespace_index_locked, which traverses state.raw.values() to reconstruct one namespace bucket (store.rs:189-230). A mutation to one grant therefore visits grants belonging to every unrelated namespace while holding the store-wide write lock.

ReferenceGrantHandler::parse calls upsert for each processed grant, and on_delete calls remove. ResourceProcessor::process_resource_inner invokes parse even for a successfully relisted unchanged object; the current source-scoped snapshot contract deliberately enqueues unchanged keys on successful relists. Many grants distributed across independent namespaces therefore undergo repeated whole-store visits even when each changed namespace contains one grant. The same owner serializes ReferenceGrant reconciliation, and its write guard excludes all permission/revision readers during each scan.

This is a cold-path structural cost, not a request-path defect, measured performance regression, availability incident or unbounded leak. It differs from copying one small resource or cloning a few Arc handles: a namespace-local index update depends on the total unrelated grant population. The index is already intended to isolate authorization lookup by target namespace, but its mutation path does not preserve that isolation.

## Bounded improvement and alternatives

Keep State and its single lock. Update the index using the old raw entry returned by insertion/removal and the new entry, touching only the old and new target namespace buckets. Preserve exact raw-key identity in the private indexed representation so replacements and deletes update precisely the right slot. A bucket keyed by the raw grant key is one feasible shape; record its additional key/index memory cost before adopting it. No additional authoritative grant store, writer task, event queue or authorization matcher is needed.

Keep replace_all as the explicit whole-store construction path and publish both maps together. Do not replace the one-lock correction with split locks. Preserve the existing Vec-returning lookup interface if useful by projecting cloned Arc handles from the bucket; do not move resource bodies or introduce request-level copies.

An alternative is retaining the existing reconstruction until representative cluster-scale evidence justifies the modest private index change. The review provides a source-proven amplification mechanism and a local ownership-preserving improvement, without claiming a numerical win or demanding a broad cache framework. The verifier must assess whether that structural value clears the cold-path review bar.

## Required semantics and migration seams

Maintain exact canonical GrantQuery matching, same-namespace policy, concrete target names, OR across whole grants, permission and revision from one snapshot, stable sorted UID/resourceVersion revisions, target namespace selection, and empty-bucket removal. Keep the controller-global validation switch and canonical bidirectional dependency manager separate from authorization. Grant events still use the existing serial workqueues and dependency-before-read ordering.

Handle both previous and new namespace metadata when replacing a raw key; do not assume every public store caller passes a key derivable from the new metadata. Retain behavior for absent namespace metadata, and ensure unrelated buckets retain their original Arc entries. A namespace-keyed index is derived state and must not become a second schema, resource cache or source of truth. No changes to Gateway serving, request-pinned credentials, external revocation guarantees, status IO, observability or transport are proposed.

## Evidence, prior decisions and future verification

Primary: edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_grant/store.rs, SHA256 95aafe5df000a670d514acae397292f2fd7cefc9a9125a1baaadbde5b3ef2583. PART-421 records full implementation, actual store/validator/event/handler tests, runtime reset/registration, canonical matcher and current queue/caller reads.

Original ReferenceGrant architecture, dependency-before-authorization regression guard, controller-global validation decision, source-scoped snapshot guard and cold-path performance review methodology were inspected. The small-object-clone page correctly rejects isolated cold copies; this recommendation concerns whole-inventory traversal per namespace-local mutation, not resource cloning. The pending resource-reference-contract specification owns reference identity and canonical matcher migration, not this store index update. ARCH-099 concerns a distinct policy timestamp owner and guarded projection; it neither changes nor fixes this derived grant index.

Future verification should retain current CRUD, exact permission, revocation/replacement revision and handler event tests, and add multiple namespaces with multiple grants, same-key replacement, removal of one and the last grant, replacement across metadata namespaces, absent namespace metadata, and full replacement. A distinguishing source check should show single-entry updates avoid traversing unrelated raw entries while raw/index mutation remains within one guard. Existing tests were read; none was executed in this review.
