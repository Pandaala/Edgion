---
name: precompiled-backend-lookup-keys
description: Review AI backend lookup identity ownership and borrowed request planning projections.
knowledge_type: regression-guards
---

# Precompiled AI backend lookup keys

## Conclusion and scope

The immutable compiled route target owns the namespace/name lookup key alongside
its authoritative typed policy target. Request planning borrows the key for
inspection and snapshot lookup; retained attempt candidates and peer contexts
keep owned keys at their established lifetime boundaries. Lookup identity never
proves ReferenceGrant permission.

## Rationale and evidence

ARCH-008 identified repeated formatting in `ai/http/proxy.rs::ai_candidate_spec`
and unconditional key/runtime copies in temporary passthrough inputs.
`backends/runtime.rs::CompiledAiTarget` now has private fields derived by the
canonical effective-reference constructor. Its immutable getters keep typed and
lookup identities together. Contract validation, invalid-share admission, model
listing, composite checks and continuation inspection use borrowed `&str` keys.
Candidate projections borrow the locally pinned route generation; ready plans
retain their own key and logical handle. Passthrough consumes a lazy iterator of
borrowed weight/runtime facts, short-circuiting Reject and canonical rewrites.

Separate passes remain necessary: denied/missing declared shares, model-route
promotion and provider/TPM publication-skew checks have different contracts. Raw
continuation slots and bindings remain counted before filtering; final dispatch
retains authorization rechecks. The [backend architecture](../../../01-architecture/02-gateway/07-backends.md)
and [graded hot-path rules](../../../00-design-rules.md#u-gateway-hot-path-performance--bounded-memory)
provide the ownership context.

## Alternatives and limits

Pass fusion would risk admission and continuation semantics. Another registry
would split generation ownership. A retained String copy is deliberate where the
plan/context outlives its temporary projection. No throughput or total-request
allocation improvement is established by the source change alone.

## Verification and re-evaluation

Unit regressions cover namespace derivation, typed identity equality, retained
old generations, repeated key pointer reuse and zero measured allocations in
candidate lookup projections. Passthrough checks cover disabled-path laziness,
missing entries, provider restrictions, nonpositive weights and the empty-set
case. Existing model-list, captured-skew and continuation regressions remain.
Fresh execution results are recorded in the implementation task; this page does
not substitute historical acceptance for current verification.

Re-evaluate when target identity, snapshot lifetime, retained candidate shape,
ReferenceGrant authorization, continuation fences or passthrough semantics change.

## Resolution references

- ARCH-008: implemented ownership correction; independent result approval and
  fresh-check completion are tracked in
  [the implementation task](../../../../tasks/pending/architecture-ai-backend-lookup-key/architecture-ai-backend-lookup-key.md).
