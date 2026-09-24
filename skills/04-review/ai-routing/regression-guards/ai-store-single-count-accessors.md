---
name: ai-store-single-count-accessors
description: Use when reviewing AI backend and adaptive telemetry store accessors.
knowledge_type: regression-guards
---

# AI stores expose one count and key accessor per snapshot

## Conclusion and scope

The AI backend store exposes `count()` and `keys()` for its activatable
snapshot. Adaptive telemetry exposes `count()` for its state snapshot. The
former `count_all()` / `keys_all()` and `len()` methods returned the same data.

## Rationale and evidence

The duplicate methods in `ai/backend_store.rs` and `ai/adaptive.rs` had no
different scope. The Admin backend-list path and adaptive tests now use the
remaining names; snapshot filtering and lifecycle are unchanged.

## Verification and re-evaluation

Re-evaluate only if a second, differently scoped population is introduced.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-008`.
