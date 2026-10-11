---
id: ARCH-028
status: confirmed
kind: architecture-debt
priority: P3
primary_partition: PART-358
independent_verification: confirmed by /root/verify_ip_union
---

# Evaluate inline and referenced IP matchers through one borrowed union

## Concrete data-path cost and ownership

StreamIpRestriction::on_connection builds borrowed reference SmallVecs from one
captured ConfigData snapshot, then calls ConnectionIpRestrictionRules::check_ip_with_refs.
That resource helper creates a fresh Vec per deny/allow side to prepend its inline
matcher before calling canonical evaluate_ip_access_multi. Nonempty sides incur
heap allocation even for an inline-only connection. GlobalConnectionIpRestriction's
ProfileRules helper repeats that projection; TLSRoute uses IpRestrictionConfig's
equivalent helper. All three delegate to the same decision algorithm, so the
algorithm itself is already unified.

HTTP IpRestriction instead combines inline and referenced borrows directly in
SmallVecs. The stream wrappers already hold borrowed collections, but the next
layer materializes another owned collection merely to meet a slice interface.
This is avoidable per-connection temporary ownership and three repeated projection
implementations, not matcher cloning, measured throughput loss or an ACL bug.
Empty sides need not allocate; reference SmallVecs can independently spill above
their inline capacity. No claim of an allocation-free complete path follows.

## Bounded improvement and alternatives

Let the canonical evaluator consume ordered borrowed matcher iteration, and
delegate inline.iter().chain(reference.iter().copied()) from these helpers.
Keep one deny/allow/default decision body. Adapt existing slice callers without
adding a second algorithm, new trait hierarchy, dynamic dispatch or request cache.
Alternatively, a shared inline-capacity borrowed collection projection removes
the common small-case allocations while retaining the slice API; it still copies
references and spills for larger unions. Lazy ordered iteration avoids that
intermediate owner entirely.

Necessary supporting changes belong only to the canonical Resources helper and
its three wrappers, plus relevant Gateway callers/tests as required. This is
evidence-backed supporting work across the crate boundary, not a broader Resources
cleanup. The stream initialization wrappers and downstream result mapper remain
separate protocol policies.

## Invariants, prior decisions and future checks

Retain one captured snapshot for Selector and all IpLists, lazy no-reference
snapshot omission, inline-before-reference order, configured reference order,
deny precedence, authored allow intent when every reference is missing/denied,
partial-good compilation, default action and matched group Arc identity. Keep
group-name diagnostics, bounded metrics and the canonical stream result mapper;
do not retain snapshot borrows across unrelated awaits or add request tracing.

The original ConfigData atomic-publication record explicitly documents stream
helper Vec allocation as a limit. It does not require that representation.
The accepted stream initialization duplication decision covers cold unrelated
validators, not these identical borrowed matcher projections. The earlier
decision algorithm consolidation is already implemented and is not re-reported.
No current task was found for this bounded root.

Future checks should compare ordered mixed inline/reference decisions, both
matched group names, missing-reference whitelist denial, reference-free default,
retained snapshot generation and larger unions. Inspect or measure allocations
separately if performance claims are desired; existing source-string tests alone
do not prove allocation behavior. This review read implementations and selected
tests without executing them. Supporting hashes are recorded in partition and
independent verification receipts.
