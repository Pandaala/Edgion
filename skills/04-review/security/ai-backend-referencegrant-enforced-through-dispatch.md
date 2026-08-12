---
name: ai-backend-referencegrant-enforced-through-dispatch
description: Use when reviewing cross-namespace EdgionBackend backendRefs, AI candidate eligibility, or ReferenceGrant enforcement on the data path.
---

# AI backend ReferenceGrant decisions are enforced through dispatch

## Security invariant

An EdgionBackend backend is usable only when it is classified as
`CompiledBackendTarget::EdgionBackend` and its Controller-stamped `ref_denied` field is absent.
`CompiledBackend::is_permitted_ai_backend` is the canonical predicate for this invariant.

The check exists at three connected boundaries:

1. AiProxy maps a denied backend's declared weight into the Gateway API invalid-weight gate.
2. Denied backends are not materialized into `AiPlanCandidate` values or looked up in the
   cluster-wide `AiBackendStore`.
3. Dispatch resolves the candidate's stable backend slot and rechecks the predicate before
   preparing provider headers or a request body.

This applies equally to direct HTTPRoute rules and flattened delegated rules because both preserve
the Controller's `ref_denied` stamp in the compiled backend schema.

## Why status is insufficient

`ResolvedRefs=False/RefNotPermitted` is an operator signal, not a data-plane authorization gate.
Partially valid routes remain programmed, and the AI path bypasses the generic Service selector
that already enforces denied refs. The AI-specific path must therefore consume the stamp itself.

## Rejected alternatives

- Match only the backend kind: the target Namespace/name can resolve in the cluster-wide service
  store even when ReferenceGrant denied it.
- Drop the whole route in the Controller: Gateway API invalid backend weights have proportional
  failure semantics, and valid siblings must remain usable.
- Check only during candidate construction: the dispatch recheck is a low-cost fail-close boundary
  against future plan-construction regressions.
- Remove denied weight before the invalid-weight gate: that silently redistributes traffic instead
  of preserving the Gateway API invalid-backend share.
