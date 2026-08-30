---
name: delegated-extensionref-remains-child-local
description: Use when reviewing HTTPRoute delegation findings about which Namespace resolves parent and child ExtensionRef filters after flattening.
---

# Delegated ExtensionRefs remain local to the declaring route

## Decision

An ExtensionRef is a local object reference. A parent HTTPRoute filter resolves EdgionPlugins in
the parent Namespace; a delegated child's filter resolves EdgionPlugins in the child Namespace.
Controller flattening preserves this origin in `LocalObjectReference.resolved_namespace`, and the
Gateway consumes the stamped Namespace when compiling the shared plugin runtime.

Parent and child plugin chains still concatenate in parent-first order. Locality controls lookup;
it does not change execution order or permit an explicit cross-Namespace plugin reference.

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
