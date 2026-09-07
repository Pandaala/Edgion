---
name: gateway-stream-plugins-same-namespace
description: Use when reviewing proposals to allow a Gateway annotation to attach an EdgionStreamPlugins resource from another namespace.
status: fixed
finding_id: gateway-stream-plugins-same-namespace-reference
closed: 2026-08-03
knowledge_type: regression-guards
---

# Gateway StreamPlugins attachments are same-namespace

## Decision

`Gateway.metadata.annotations["edgion.io/edgion-stream-plugins"]` accepts only an unqualified
resource name. The Controller resolves it in the Gateway namespace. A slash-containing value makes
the Gateway-level `ResolvedRefs` condition false and is not entered into the StreamPlugins reverse
dependency index. The Gateway independently fail-closes the listener for such a value.

The former `namespace/name` behavior was not authorized by `ReferenceGrant`; existence validation
and reverse dependency tracking did not establish permission to consume another namespace's policy.

TCPRoute, TLSRoute, and UDPRoute keep their separate `name` or `namespace/name` annotation contract.
Do not infer their semantics from this Gateway-only rule.

## Sharing model

Operators who need common data across multiple Gateway namespaces should keep a local
`EdgionStreamPlugins` wrapper in each namespace and reference shared `EdgionConfigData`. That path
already enforces `ReferenceGrant` and target `visibility`, while leaving plugin composition and
failure behavior under the Gateway namespace's control.

Reintroducing shared cross-namespace executable policies requires a separate design: target-side
authorization, grant-change revalidation, an authorization result transported to the data plane,
and a Gateway-side fail-close check that cannot be bypassed by supplying `namespace/name` directly.

## Resolution references

- `gateway-stream-plugins-same-namespace-reference` — 2026-08-03.
