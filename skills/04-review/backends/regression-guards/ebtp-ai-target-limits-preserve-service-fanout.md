---
name: ebtp-ai-target-limits-preserve-service-fanout
description: Use when reviewing EdgionBackendTrafficPolicy targetRefs limits; AI-only bounds must not invalidate existing Service fan-out or duplicate-reference behavior.
status: fixed
finding_id: ctrl-02
closed: 2026-08-09
knowledge_type: regression-guards
---

# ctrl-02 — AI target limits preserve existing Service policy behavior — FIXED

## Conclusion

The 16-target and unique-target constraints apply only to homogeneous EdgionBackend policies.
Service policies retain their original unbounded fan-out and treat repeated references to the same
Service idempotently. The shared structural CRD therefore declares only `minItems: 1`; conditional
AI limits are enforced by `EdgionBackendTrafficPolicySpec::get_validation_errors()`.

Fix suggestions that truncate a policy to its first 16 targets or invalidate an entire Service
policy because of an AI-specific dependency bound are **not accepted**.

## Core Rationale

**1. New feature bounds must not silently change existing Service policy behavior**

Before EdgionBackend support, Service policies had neither a maximum target count nor duplicate
target rejection. Gateway arbitration already keys each effect by canonical Service identity, so a
duplicate reference is idempotent. Applying the new AI bounds to Service policies made an otherwise
valid policy disappear from Gateway arbitration and cleared all of its effective traffic settings.

**2. The AI bound remains enforced at the owning validation layer**

EdgionBackend policies introduce bounded AI dependency, sibling-arbitration, and status work, so
they still accept at most 16 unique targets. A plain OpenAPI array `maxItems` cannot express that
condition without also limiting Service policies; the shared resource-layer validator can because
it first resolves the homogeneous typed target kind. Controller and Gateway both consume that same
validator.

**3. Atomic invalid-policy behavior remains unchanged**

An invalid AI policy is still rejected as a whole. The fix does not add truncation, partial-good
application, or another failure-policy shape. Mixed target kinds and invalid target identities
remain errors independently of the conditional AI count and uniqueness checks.

## Fix Suggestions Not Accepted

- Keep the CRD-wide `maxItems: 16` and raise the value — any fixed shared maximum still changes the
  established Service contract without an architectural need.
- Truncate to the first 16 targets — silently creates order-dependent partial policy application.
- Convert duplicate AI targets to warnings — weakens the bounded, canonical AI dependency contract.

## Re-evaluation Triggers

Re-open this decision only if:

- Service target fan-out receives its own explicit, product-approved resource limit with migration
  and failure semantics.
- The CRD adopts a structural conditional schema that can express target-kind-specific array
  constraints consistently across Kubernetes, filesystem, and etcd modes.

## Reference Cases

- ctrl-02 (review finding, fixed 2026-08-09)
- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
- `skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`

## Resolution references

- `ctrl-02` — 2026-08-09.

## Policy target owner identity

The actual BTP/EBTP field loans in `refs::policy_fields` resolve omitted owner
namespace to `default`; explicit empty namespace is invalid. Raw policy keys,
queue ownership and exact deletion remain unchanged. Controller winner/status
checks use the raw policy key separately from semantic target identity.

Malformed EBTP target identities reject the whole policy before target or BTP
hostname reads. Canonical valid siblings retain recovery dependencies and ancestor
identity, but no partial conflict eligibility or resolved marker is published.
Service fan-out/duplicates remain unrestricted; only EdgionBackend targets carry
the existing 16-target and uniqueness limits. BTP still requires one eligible
target and preserves invalid exact/whole occupancy for canonical target slots.
Gateway target indexes share these loans without changing atomic publication,
precedence, captured snapshots, or TLS/material ownership.
