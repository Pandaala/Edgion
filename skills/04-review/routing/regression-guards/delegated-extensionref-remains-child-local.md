---
name: delegated-extensionref-remains-child-local
description: Use when reviewing HTTPRoute delegation findings about which Namespace resolves parent and child ExtensionRef filters after flattening.
knowledge_type: regression-guards
---

# Delegated ExtensionRefs remain local to the declaring route

## Decision

An ExtensionRef is a local object reference. A parent HTTPRoute filter resolves EdgionPlugins in
the parent Namespace; a delegated child's filter resolves EdgionPlugins in the child Namespace.
Controller flattening preserves this origin in `LocalObjectReference.resolved_namespace`, and the
Gateway consumes the stamped Namespace when compiling the shared plugin runtime.

Parent and child plugin chains still concatenate in parent-first order. Locality controls lookup;
it does not change execution order or permit an explicit cross-Namespace plugin reference.

## Owning Controller boundary

HTTP and gRPC Controller lookups borrow the actual declaring Route and original
rule/backend/filter slot. The loan retains its actual UID and generation before
flattening clones; a private receipt records that declaration while the root's
raw ResourceRef independently owns wakeups. All valid child references register
before later drops. Structurally eligible scope/transcoder checks then read the
desired plugin generation through `get_raw_any`, never a stale processed-cache
substitute. Missing bundles keep their recovery edge. Parse clears once and
later registration only adds to that reconcile's set.

The existing transport stamp carries locality, not original child UID/generation.
Effective target-only loans validate the published root and actual effective
field, including the stamp's namespace syntax; they cannot claim stronger child
provenance or mint a Grant query. Authored views ignore supplied stamps.
An omitted root namespace remains absent in wakeup keys, separately from the
canonical `default` namespace used by the local plugin target.

The Controller migration is R3a1. Gateway early global/CORS/WAF/access-log lookup
and generation capture remain R3a2 work; this page does not certify those paths.
Re-evaluate source binding, stamp handling, query ordering and repair behavior
when these consumers or the published transport change.

## Compatibility posture

Edgion is pre-release and carries no backward-compatibility burden. The earlier behavior that
looked up a child's ExtensionRef in the flattened parent Namespace was incorrect, so it does not
receive a fallback, alias, or migration shim. User and architecture documentation state the
resource-local rule directly rather than presenting the correction as a production upgrade path.

## Rejected alternatives

- Fall back to the parent Namespace when the child-local object is absent: this makes a local
  reference depend on object presence and can silently select an unintended policy.
- Resolve every flattened filter in the parent Namespace: this erases the declaring resource's
  identity and prevents child teams from owning their plugin configuration.
- Add an explicit Namespace field to ExtensionRef: the current reference is intentionally local;
  cross-Namespace authorization would require a separate designed API and ReferenceGrant model.

## Resolution references

- `gw-11` — 2026-08-09 (docs clarification; no compatibility shim).
