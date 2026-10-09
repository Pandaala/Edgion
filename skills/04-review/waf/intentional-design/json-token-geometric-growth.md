---
name: json-token-geometric-growth
description: Review the accepted bounded geometric growth policy for JSON token scratch.
knowledge_type: intentional-design
---

# Retain bounded geometric growth for JSON token scratch

## Conclusion and scope

Keep the current JsonStream token growth policy. The user explicitly declined
the conditional cached-source growth-hint optimization after reviewing its
potential spare-capacity cost. This is an accepted optimization tradeoff, not a
claim that a hint was implemented, measured ineffective, or proved unnecessary
for every workload.

## Rationale and evidence

JsonStream::token is SmallVec<[u8; 1024]>. Contiguous unescaped strings can be
borrowed; buffered tokens reuse capacity within the parser. retain_token_bytes
uses json_scratch::growth_target only when required length exceeds capacity,
then reserves fallibly and accounts actual capacity. Geometric growth is capped
by the remaining retained budget. Inline capacity is not heap-budget credit.

JsonChecker::with_source_len currently supplies reducer hints, not token hints.
Keeping this separation avoids another token-planning policy and preserves the
existing allocation, copy-work and failure trajectories. The accepted cost is
unused capacity after growth and copies when subsequent growth is necessary.
Its production frequency and net performance impact have not been measured.

The existing [fused JSON guard](../regression-guards/fused-json-inspection.md)
owns grammar, source lifetime, admission and publication contracts; this page
records only the capacity-policy choice.

## Alternatives and limits

The declined alternative caps a geometric growth target using complete cached
source length, only when expansion is already necessary. It must never eagerly
reserve a whole body, trust Content-Length, shrink reusable capacity, reduce the
inline default, or reject required bytes solely because they exceed a hint.
An exceeded hint must restore geometric growth to avoid repeated exact-plus-one
allocation and copying. Inspection-session source-length parity remains separate.

The retained-byte budget excludes allocator overhead and transient reallocation
overlap. This decision neither changes that contract nor claims it bounds total
process heap. Any future experiment must report actual capacities, allocation
counts, requested bytes, copies and simultaneous peaks separately.

## Verification and re-evaluation

Current source and existing token spill, exact-limit and large-then-small reuse
tests were inspected for this decision; no new runtime test or benchmark result
is claimed. Reconsider if representative buffered-token workloads demonstrate
material spare-capacity or allocation costs, or if source ownership, token reuse
or the memory-admission contract changes. Earlier experimental design material
does not authorize implementation of this declined optimization.

## Resolution references

- waf-json-token-growth-hint: conditional P2 optimization declined by the user;
  closed without production changes after independent review and project checks.
