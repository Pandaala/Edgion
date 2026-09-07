---
name: session-persistence-two-state-inheritance-and-k8s-strict-scope
description: Use when reviewing proposals to add an explicit three-state (unset/disabled/configured) session-persistence override so a delegated child can turn off inherited persistence, or to make session_persistence.strict operator-settable under Kubernetes; covers why the delegation model stays two-state and why strict is intentionally File/Etcd-only.
status: accepted-tradeoff
finding_id: session-persistence-disable-and-strict-semantics
closed: 2026-07-18
knowledge_type: intentional-design
---

# Session persistence: two-state delegation inheritance + File/Etcd-only strict — DESIGN-TRADEOFF (won't build three-state)

## Conclusion

Session persistence intentionally keeps a **two-state** override model in route
delegation — `unset` (field absent ⇒ inherit nearest parent) and `configured`
(local `sessionPersistence` ⇒ override the inherited one). An explicit third
"disabled / stop-inheritance" state is **not** added, and `session_persistence.strict`
stays an **Edgion File/Etcd-only** field (the vendored standard Gateway API CRD
prunes it, so Kubernetes operators cannot set it and get default non-strict
behavior). No CRD fork, no policy-attachment resource.

Fix suggestions of the form "add an explicit `disabled` state so a delegated child
can turn off parent session persistence", "model unset/disabled/configured as a
oneof/enum on `SessionPersistence`", "patch the vendored gateway-api CRD to add
`strict`", or "add an `EdgionSessionPersistencePolicy` targetRef resource to carry
strict in K8s" are **not accepted** as of this decision.

## Core Rationale

**1. `session_persistence` is not special — it inherits exactly like `timeouts` and `retry`.**

In HTTPRoute/GRPCRoute delegation flattening (`route_delegation.rs` /
`grpc_route_delegation.rs`), `InheritedAttrs` carries three rule-level attributes
with one uniform "child-first-else-inherited" rule:

```rust
struct InheritedAttrs {
    timeouts: Option<HTTPRouteTimeouts>,
    retry: Option<HTTPRouteRetry>,
    session_persistence: Option<SessionPersistence>,
}
```

The merge is `rule.session_persistence.clone().or_else(|| attrs.session_persistence.clone())`.
None of the three has an explicit opt-out — a child cannot turn off an inherited
timeout or retry either. Adding a three-state disable *only* for session
persistence would make it inconsistent with its two siblings. A real per-attribute
opt-out should be a single uniform mechanism across all inherited attributes, which
is a much larger feature — deferred until an operator actually needs it.

**2. Two-state is the only representation expressible under the standard CRD, and forking it is worse.**

HTTPRoute/GRPCRoute use the upstream Gateway API CRD vendored verbatim
(`config/crd/gateway-api/gateway-api-experimental-v1.5.0.yaml`), which is
structural and prunes non-standard fields. Its `sessionPersistence` block has no
`disabled` marker and no `strict`. An empty `sessionPersistence: {}` means "enabled
with default cookie persistence" per Gateway API — it cannot be repurposed as
disabled. So an explicit disable/strict in K8s would require either patching a
file named after an upstream release (re-patched every bump, diverges from
conformance) or a whole new policy-attachment resource (targetRef + sectionName +
precedence). Neither is justified by the (currently hypothetical) demand.

**3. strict already works where it can — it is a real, implemented feature, not a claim.**

`strict` is fully wired in the gateway runtime (route-09, see
[sp-strict-mode-fail-close.md](../regression-guards/sp-strict-mode-fail-close.md)): in-attempt 503 on a
resolved-but-unservable sticky target, cross-attempt re-pin. It is honored in
FileSystem/Etcd modes and over conf_sync to the data plane. The only gap is that
K8s cannot *set* it (CRD prunes the field), so K8s gets default non-strict. That is
a documentation/scope boundary, not a defect — and deleting a working, tested
feature just to erase a doc gap would destroy real value.

## Fix Suggestions Not Accepted

- Add an explicit `disabled` third state (enum/oneof) to `SessionPersistence` — rejected: inconsistent with `timeouts`/`retry`, which have no opt-out; a uniform per-attribute opt-out is a separate future feature.
- Patch the vendored gateway-api CRD to add `strict` / a disabled marker — rejected: diverges from upstream conformance, re-patched on every Gateway API bump.
- Add an `EdgionSessionPersistencePolicy` (GEP-713 policy attachment) to carry strict/disable in K8s — rejected: a whole new resource type + targetRef/sectionName resolution + precedence for demand that does not yet exist.
- Delete `strict` to remove the "unsupported in K8s" wart — rejected: it is a real, implemented, tested feature (route-09) that works in File/Etcd; only its K8s configurability is out of scope.

## Re-evaluation Triggers

Reopen only if:

- Operators demonstrate a concrete need for a delegated child to turn off inherited persistence (and, by consistency, inherited timeouts/retry) — then design one uniform per-attribute opt-out, not a session-persistence special case.
- Operators demonstrate a concrete need to configure `strict` (or explicit disable) from Kubernetes — then evaluate the policy-attachment resource path (not a CRD fork).
- The delegation model changes so that `session_persistence` no longer inherits alongside `timeouts`/`retry`, invalidating the consistency argument.

## Reference Cases

- Origin task (closed without a spec/build): `tasks/todo/session-persistence-disable-and-strict-semantics.md`, brainstorming 2026-07-18. Envoy `StatefulSessionPerRoute` oneof (`disabled` | `stateful_session`) was the reference model; rejected because Edgion delegation inheritance (an Edgion extension, not in upstream Gateway API) makes the three-state cost/benefit different.
- Related fixed finding: [routing/sp-strict-mode-fail-close.md](../regression-guards/sp-strict-mode-fail-close.md) (route-09) — strict runtime fail-close semantics.
- Delegation inheritance model: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation.rs` (`InheritedAttrs`, `flatten`), mirrored in `grpc_route_delegation.rs`.

## Resolution references

- `session-persistence-disable-and-strict-semantics` — 2026-07-18 (⚠️ accepted tradeoff / won't-build).
