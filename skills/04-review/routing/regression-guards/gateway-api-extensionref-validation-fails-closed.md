---
name: gateway-api-extensionref-validation-fails-closed
description: Use when reviewing HTTPRoute or GRPCRoute ExtensionRef discriminator, target, status, delegation, or Gateway fail-closed behavior.
knowledge_type: regression-guards
---

# Gateway API ExtensionRef validation fails closed

## Conclusion and scope

HTTPRoute and GRPCRoute filters use one shared validation decision for the
`type`/`extensionRef` union and Edgion's supported target. `type: ExtensionRef`
requires an `extensionRef` payload, every other type forbids that payload, and
the supported plugin target is exactly `edgion.io/EdgionPlugins`. An empty
group means the Kubernetes core API group; it is not an alias for `edgion.io`.

The Controller applies this rule to rule and backendRef filters, including raw
child routes during delegation flattening. Invalid rules are dropped by exact
index. A surviving sibling produces `PartiallyInvalid=True` with a message
starting with `Dropped Rule`; when no rule survives, the attachment is
`Accepted=False` and `PartiallyInvalid` is absent. The Gateway independently
seals each malformed runtime with the canonical generic 500 request gate. A
rule-scoped gate runs before CORS preflight, ordinary plugin execution, or
upstream processing; a backend-scoped gate runs only after that backend is
selected and before forwarding. Global and Gateway request-ID plugins may
already have run in Pingora's early hook before route matching identifies a
malformed route. A source-only AI diagnostic does not imply a dropped or
fallback rule and must not be represented by `PartiallyInvalid`.

## Rationale and evidence

Gateway API's HTTPRoute and GRPCRoute filter CEL declarations require the
discriminator and payload in both directions. `LocalObjectReference.group`
also defines an empty value as the core API group. The standard's
`PartiallyInvalid` condition is positive-only for implementations that drop
invalid rules, and its message prefix identifies that behavior.

`edgion_resources::resources::extension_ref::validate_extension_ref_filter`
owns the shared decision. The HTTP and gRPC Controller handlers consume it for
source rules; delegation calls those same validators against raw children. The
Gateway HTTP and gRPC filter builders consume it before constructing any
filter chain. The matched rule runtime participates in the same preflight gate
as global, Gateway, and referenced plugin runtimes, preventing a rule-level
malformed filter from letting an OPTIONS request return 204 first. BackendRef
filters remain backend-scoped as Gateway API requires: they execute only after
that backend is selected for forwarding, so a locally terminated preflight does
not execute them. On the direct-Gateway defense path, this also prevents one
invalid weighted backend from poisoning valid siblings. Filesystem and etcd
input still passes through the Controller's admission-parity rule validation,
while Kubernetes normally rejects the malformed union through CEL. When plugin
policy adds an observational bypass note to a separately invalid runtime,
`RejectInvalidConfig` remains first; otherwise the preflight fast path would
mistake the note for an ordinary runtime and skip the gate.
Explicit unsupported targets retain the existing
`ExtensionRefFilter` attribution when malformed data bypasses the Controller;
only a missing or discriminator-mismatched payload uses the generic invalid
configuration gate.

## Alternatives and limits

Silently ignoring an absent payload is unsafe because a policy-bearing filter
disappears while traffic continues. Treating an empty group as `edgion.io`
would contradict Gateway API reference semantics. Rejecting an entire route
when a valid sibling remains would discard the standard's partial-validity
behavior.

Kubernetes admission normally rejects the malformed union first. The
Controller and Gateway checks remain necessary for filesystem, etcd, conf-sync,
and defense-in-depth paths. This guard does not change dangling
`EdgionPlugins` lookup behavior, backend-scoped WAF restrictions, response
streaming, or access-log ownership.

## Verification and re-evaluation

Focused unit coverage exercises the shared decision matrix; HTTP and gRPC
rule/backendRef status; delegated raw-child filtering; generic Gateway 500
behavior for rule and backend scopes; and policy-bypass ordering ahead of
preflight. Re-evaluate if Edgion supports another
ExtensionRef group/kind, Gateway API changes the union or partial-validity
contract, or the route ingestion architecture changes.

Fresh verification on 2026-09-12 passed the focused resource, Controller,
delegation, publication, adaptive-routing, ExtensionRef runtime, and real
OPTIONS-session tests. `cargo check --workspace --all-targets`, `cargo clippy
--workspace --all-targets`, the agent-doc validator, SSA force guard, metrics
inventory guard, and Pingora source-policy guard also completed successfully.
Clippy reported only the repository's existing non-fatal warnings. Independent
final review returned LGTM after the preflight, policy-bypass ordering, backend
scope, and status corrections.

## Resolution references

- `fail-closed-missing-gateway-api-extensionref-configuration` — fixed 2026-09-12.
