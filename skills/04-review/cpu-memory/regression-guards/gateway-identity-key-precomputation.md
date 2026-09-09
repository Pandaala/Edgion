---
name: gateway-identity-key-precomputation
description: Use when reviewing GatewayInfo identity ownership and per-request Gateway key allocations.
knowledge_type: regression-guards
---

# Gateway identity keys are precomputed per shared generation

## Conclusion and scope

`GatewayInfo::new` computes its canonical key once using `format_gateway_key`.
The namespace, name and cached key are private; `gateway_key()` borrows the
stored string. Raw `namespace()` preserves `None` versus `Some("")`, while the
existing metrics namespace getter normalizes both to an empty string. The
default identity and the original Debug field projection remain unchanged.

This is immutable identity caching, not a cache of TLS credentials, policy
decisions, route authorization or listener configuration.

## Rationale and evidence

Previously `gateway_key()` formatted or cloned a String on ordinary HTTP/gRPC
Service peer attempts, DirectEndpoint peer construction and AllEndpointStatus
endpoint collection. Those callers now borrow the existing key. The ordinary
peer path clones only the existing GatewayInfo Arc so the key remains available
while the request context is mutated or DNS resolution is awaited.

`ListenerGatewayInfoStore::rebuild` still owns a separate String for its cold
index entry. Flat and indexed views share the same GatewayInfo Arc, published
in the existing single snapshot. Retaining an old request's Arc keeps its
original identity stable after a store rebuild.

Canonical authority: [hot-path design rules](../../../00-design-rules.md),
[Rust ownership rules](../../../03-coding/02-rust-coding-rules.md) and the
[single-snapshot guard](../../concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md).
The [backend identity selection guard](../../tls/regression-guards/backend-client-identity-tls-selection.md)
continues to govern dynamic TLS lookup and policy identity precedence.

## Alternatives and limits

The cost is one retained key String per Gateway/listener generation, including
older generations retained by in-flight requests. No new global cache, lock,
snapshot load, metric or log is introduced. The cold index copy is intentional.
This correction does not assert a measured latency or RSS improvement, or that
the complete peer-selection path is allocation-free.

The route parentRef lookup key is a separate precomputed object. Gateway handler
publication and TLS-store cleanup are not changed by this ownership correction.

## Verification and re-evaluation

Inline `runtime::gateway_info::tests` cover absent/empty namespaces, empty names,
namespace isolation, metric getters, default values, original Debug content and
stable borrowed key storage across shared Arc handles. The listener-store tests
cover flat/index Arc sharing and retained old identities after replacement.
Pointer assertions establish shared backing storage, not an allocation profile.

B0217 implementation verification passed the Gateway library check and targeted
unit filters: identity (3), listener store (10), runtime matching (41), peer
builder (15), HTTP upstream peer (92), and AllEndpointStatus (18). Agent-doc
validation passed. These are local unit/compile checks, not fresh network or
whole-workspace test evidence.

Re-evaluate if identity becomes mutable, a constructor bypass is introduced,
GatewayInfo becomes serialized, or a request consumer starts owning the key
again. TLS credential rotation remains a dynamic store concern; never extend
this identity cache to resolved credentials or policy results.

## Resolution references

- Gateway quality campaign B0217: immutable GatewayInfo identity precomputation.
