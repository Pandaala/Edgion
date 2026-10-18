---
name: consistent-hash-ring-seenset-no-heap
description: Review consistent-hash visited-index ownership, inline first-candidate tracking and dense fallback allocation.
knowledge_type: regression-guards
---

# Consistent-hash visited-index storage

## Current contract

`ConsistentHashRing::select` uses a private `SeenSet`, not a per-call HashSet.
At most 128 backend slots use a stack u128 mask. Larger lists initially retain
one index inline; duplicates of that index remain allocation-free. Visiting a
second distinct index allocates one dense Vec<bool>, marks both indices, and
transitions to ordinary vector membership checks. Vec<bool> is a dense vector,
not a promise of bit-packed storage.

This avoids tracker allocation for a healthy primary, zero selection budget or
one rejected candidate without further attempts, even on large lists. Ring
construction, caller-owned state and arbitrary health predicates are outside
that allocation claim. Larger fallback walks still allocate once.

Indices remain original backend-list indices, including gaps from non-Inet
entries. Size the vector from `backends.len()`, not the number of distinct ring
addresses. The select loop, borrowed winner, health checks and distinct-backend
budget remain unchanged. Preserve the separate
[duplicate-node budget and termination contract](consistent-hash-duplicate-node-budget.md).

## Evidence and limits

`seen_tracker_delays_large_allocation_and_preserves_membership` covers 128,
129 and 200 slots, first insertion at the highest index, duplicates and
promotion retaining both original entries. The large-ring regression checks
zero tracker allocations for primary/zero-budget/one-attempt cases and one
allocation while rejecting all 200 unique candidates exactly once. Existing
weighted fallback, non-Inet, deterministic selection and borrowed-storage tests
remain relevant. No throughput or fleet-size distribution is established.

Re-evaluate when membership representation, index projection, health predicate
calls, iteration budget or ring traversal changes. This record cannot suppress
new allocation evidence at a different owner or on the dense fallback path.

## Historical resolution

lb-F003 (2026-05-24) replaced HashSet with the u128 mask and an eagerly allocated
Vec<bool> above 128 slots. The subsequent lazy-first-index optimization removes
that eager allocation until a second distinct candidate is visited. Historical
results are not fresh verification of later changes.
