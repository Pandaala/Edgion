---
id: ARCH-105
status: candidate
kind: behavior-defect
priority: P3
partition: PART-403
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-105: Bind local credential-manager results to the source snapshot that produced them

## Current ownership

CredentialSourceRegistry stores desired sources and manager results under one RegistryState lock, but results are keyed only by logical namespace/name. CredentialManagerResult carries observed_generation without a source UID or input-revision binding.

forget_source removes the desired source and result together. However, replace_result unconditionally inserts a result without checking whether its producing source still exists. result returns it by logical key alone.

The asynchronous credential manager captures DesiredCredentialSource snapshots and retains separate SourceState. publish_manager_result constructs a result using the supplied source's generation and unconditionally calls replace_result. Its source_is_current and consumer_is_current condition gates memory cleanup only; a false condition does not prevent the local result write.

## Concrete lifecycle witnesses

First, a manager tick can capture source A while the serial LinkSys worker later deletes A and completes forget_source. When the captured tick reaches publish_manager_result, source_is_current is false, but replace_result restores the deleted source's result. LinkSys status processing can then find no raw resource and skip processing without repeating forget_source. A subsequent manager tick removes its private state for the absent source but does not remove this restored registry result. This is orphan local bookkeeping, without a claim of an unbounded leak or stale token publication.

Second, LinkSysHandler::parse removes a previous desired source and registers a newly eligible source. It removes the previous result only when no new source was registered. A same-name replacement with a different UID and the same metadata.generation can therefore retain the old result. LinkSysHandler::update_status filters only observed_generation and can project the old source's credential state, publication identifier, refresh times and counts onto the replacement resource.

There is also an actual producer-side pairing hazard: JobResult::Cleaned looks up the current desired source and the existing SourceState independently. A source replacement while cleanup is outstanding can pair the new source with old state. Adding a current-source check to replace_result alone would not prevent that old state from being stamped with the new source's generation.

## Bounded improvement

Give the existing private result slot a producing-source qualification: source UID, generation and the canonical input revision. Keep at most one result slot per logical key. The registry should compare the supplied qualification with its current desired source and perform insertion under the same RegistryState write lock. A deletion or replacement must therefore prevent an older producer from restoring or relabeling the local result.

The result read must also reject a slot whose qualification no longer matches the current desired source, including a same-name UID replacement. This avoids depending on every handler to clear results during the transient remove-and-register sequence.

Before constructing a result, publish_manager_result must verify that SourceState.identity and SourceState.input_revision belong to the supplied source. Preserve the existing source_input_revision algorithm as the single owner; if registry qualification needs that calculation, relocate or expose that existing private calculation at the narrow shared source-state boundary rather than duplicate its fingerprint logic. Compare metadata/proof inputs without cloning resolved TLS material.

These changes concern local result ownership. Retain existing worker-generation, reconciled-consumer, source-currentness, publication, Secret and Lease checks for their existing effects. No upstream I/O or external authority check belongs inside the registry lock.

## Required semantics and limits

Preserve one serial resource worker, key-only requeues, changed-result requeue suppression, current-source expiry reporting, desired/published/conflicted counts, persist-off fan-out counting, retries and last-good retained publication policy. A failed current acquisition may still report the current state's retained publication as before.

Private result qualifications must not become operator schema or conf_sync fields, and must not expose credentials or resolved TLS material through Debug, serialization or logs. Source snapshots, RetainedPublication, memory entries and request-pinned credentials keep their separate accepted ownership.

This does not establish atomic external authorization revocation, exactly-once publication, physical erasure, or a Gateway token defect. ARCH-094's service/gauge lock scope is a separate cause.

## Original knowledge and deduplication

The LinkSys architecture and credential-source-single-active-memory guard establish UID-qualified active source identity while accepting independent retry and runtime owners. credential-copy-retention.md does not authorize result metadata to outlive its source without qualification.

The pending resource-reference-contract specification and implementation explicitly retain scoped asynchronous owner/proof checks and exclude broad cross-system atomic revocation guarantees. This recommendation stays within one local registry and its actual result producer.

## Future verification

Use deterministic source snapshots rather than scheduler timing:

1. Capture a registered source and valid current SourceState, call forget_source, then attempt its actual result publication; no result may be restored.
2. Store a result for UID A, register UID B under the same logical key and generation, and verify that B cannot read A's result.
3. Exercise a cleanup completion with old SourceState and a replacement desired source; no result may be attributed to the replacement.
4. Reject a SourceState input revision that differs from the producing source's canonical revision.

Retain the existing persist-off counting and monotonic expiry regressions. Their SourceState fixtures currently leave input_revision empty; qualify those fixtures with the real canonical revision rather than weakening production ownership checks for test convenience. Existing tests and source inspection are evidence only; no tests were run for this review.
