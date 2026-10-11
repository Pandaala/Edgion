---
id: ARCH-103
status: candidate
kind: architecture-debt
priority: P3
partition: PART-408
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-103: Prune empty ConfigData reference buckets when replacing an owner's edges

## Current ownership and concrete cost

EdgionConfigDataRefManager owns a forward map from ConfigData target keys to dependent-key sets. Its add_ref creates a bucket. clear_refs_from removes a dependent from every set but never removes sets that become empty. The forward-entry removal convenience method has no production caller; ControllerRuntime reset clears the whole map, but ordinary owner replacement and deletion do not.

Actual handlers call clear_refs_from before rebuilding EdgionPlugins, EdgionStreamPlugins, Gateway and WafPolicy dependencies. For example, EdgionPluginsHandler clears its owner and registers the current RegionRoute overrideRef key. Missing targets still register dependencies so a later target creation can wake the owner. Repeatedly retarget one live plugin to new valid ConfigData names: each replacement leaves the previous now-empty target bucket. After deleting that owner, the manager can retain every historical target key although it has no remaining live edges. The referenced ConfigData objects need never have existed.

This is a source-supported retention path, not a hypothetical missed-delete event. clear_refs_from correctly removes the logical edge on every pass. The missing operation is retiring its empty bucket. Every later clear scans the historical buckets under the write lock, so unrelated owner reconciliation cost follows historical target churn rather than the current dependency graph. No latency, heap total, incident or growth rate was measured.

## Bounded improvement and alternatives

In the existing clear_refs_from critical section, remove the dependent and retain only buckets whose sets remain nonempty. Keep the public dependent-key representation and all callers unchanged. Absent and empty source buckets both return an empty vector through get_refs, so this removes obsolete ownership without changing lookup results.

Do not remove a source bucket merely because the target ConfigData is absent or deleted. Nonempty incoming edges are intentionally retained by EdgionConfigDataHandler::on_delete; recreation must wake unchanged consumers. Preserve other dependents sharing the target, idempotent add/clear, serial handler workqueues, clear-before-add replacement, exact bare versus namespaced owner wake keys and ControllerRuntime reset.

The existing BidirectionalRefManager removes empty buckets and can eventually avoid whole-map owner scans through its reverse index. Adopting it here would require changing dependent representation and several handler/test seams. That is a broader alternative, not necessary for this local retention correction. Add no periodic GC, TTL, queue, metric, framework, extra cache or request-path work. Removing entries does not guarantee shrinking the outer HashMap allocation or establish a process memory bound.

## Original knowledge and validation

The original processed-resource architecture describes reference managers as current dependency indexes. The no-GC review proves the generic BidirectionalRefManager's empty-bucket removal and clear-then-add ownership; those concrete assumptions do not apply to this separate forward-only implementation. Its accepted rejection of periodic GC remains valid. The current ConfigData delete/recreate tests explicitly require retaining nonempty incoming dependencies, and WAF bare-owner retarget/delete tests assert that get_refs becomes empty but cannot distinguish an empty retained bucket from an absent one.

Add a focused private unit test for repeated owner retargeting and deletion, inspecting that obsolete buckets are absent, another dependent survives, and a missing/deleted target's nonempty bucket still wakes its unchanged consumer after recreation. Keep tests in the established source-relative unit-test path without widening production visibility. Existing ConfigData/WAF/ProxyProtocol wake-key, grant and snapshot tests must retain their behavior. No tests, builds or runtime reproduction were executed. Current task and finding searches did not identify this empty-bucket retention correction as already tracked.
