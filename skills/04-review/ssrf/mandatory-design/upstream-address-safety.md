---
name: upstream-address-safety
description: Decision rule for upstream address safety boundary. Read this file first when reviewing paths that rewrite or select an upstream — DynamicExternalUpstream, RegionRoute, DynamicInternalUpstream, DirectEndpoint, RequestMirror, EndpointSlice/LB, etc.
knowledge_type: mandatory-design
---

# Upstream Address Safety

## Conclusion

Any path along which the Gateway data plane will establish an upstream connection must satisfy two layers of constraints simultaneously:

1. **Authorization constraint**: the requester can only select operator/route-authorized targets, e.g., `domainMap`, `backendRefs`, RegionRoute PM, RequestMirror backendRef.
2. **Address safety constraint**: the final dial address must undergo safety checks based on its source type:
   - External / DNS-style upstream (e.g., `ExternalJumpPreset`) by default rejects loopback and link-local / metadata addresses, namely `127.0.0.0/8`, `::1`, `169.254.0.0/16`, `fe80::/10`.
   - K8s Endpoint / EndpointSlice-derived upstream by default likewise rejects loopback and link-local / metadata; only the explicit integration testing mode may permit Endpoint-derived loopback for local fixtures.
   - `EdgionGatewayConfig.spec.securityProtect.allowLoopbackUpstream=true` is a GatewayClass-level operator override that only permits loopback, not link-local / metadata.

RFC1918 private addresses (`10/8`, `172.16/12`, `192.168/16`) are allowed by default; K8s Service, Pod, cross-cluster gateway, and enterprise internal APIs all rely on these addresses.

### Vendor metadata endpoints outside link-local (common-01)

The built-in classifier blocks only protocol-defined ranges. Vendor metadata
endpoints outside them (Alibaba `100.100.100.200`, AWS IPv6 IMDS `fd00:ec2::254`,
GCP IPv6 `fd20:ce::254`, Oracle IPv6 IMDS, Azure WireServer) are blocked through
the Gateway-local, additive `gateway.outbound_egress_blocked_cidrs` list, which
the shipped configuration files pre-populate and which every dial path above
inherits through `classify_ip` (`IpClass::BlockedCidr`,
`BackendAddrBlockedCidrNotAllowed`). Vendor addresses are deliberately not
hard-coded into the classifier; see
[Operator-configured egress blocked CIDRs](../intentional-design/operator-blocked-cidrs.md).
CGNAT, NAT64/6to4 embedded IPv4 and reserved ranges remain an open P2 gap tracked in
[`tasks/todo/egress-classifier-cgnat-nat64-reserved-ranges.md`](../../../../tasks/todo/egress-classifier-cgnat-nat64-reserved-ranges.md).

## Review Decision

If a path applies only the authorization constraint but does not apply the address safety constraint before the final dial, it counts as a real issue, typically rated **P2 security hardening / defense bypass**. It is not arbitrary user SSRF, but it bypasses Edgion's existing defenses against cloud metadata / link-local. Missing loopback in external/DNS-style upstreams is treated the same way.

Acceptable fix directions:

- Reuse `classify_backend_addr()` / `validate_backend_addr()` or equivalent logic.
- DNS hostnames must be checked after resolution to addresses; cannot check only the string.
- IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) must be normalized to their embedded IPv4 form before classification; otherwise mapped loopback / link-local / metadata targets bypass the IPv6 checks (fixed in backends-F003).
- The check point should be as close as possible to the final `HttpPeer` / `reqwest` / TCP dial creation, to prevent inter-plugin bypass.

Unacceptable fix directions:

- Block all private CIDRs.
- Resolve DNS only at config-validation time.
- Rely solely on `domainMap` / `backendRefs` allowlists to prove safety; allowlists are an authorization boundary, not a metadata-address defense line.

## Inventoried Paths

| Path | Current semantics | Review notes |
|---|---|---|
| `DynamicExternalUpstream` | Request value can only hit `domainMap`, but the target hostname/IP can be given by config | After resolution, must reject loopback + link-local; rejecting only loopback is not enough |
| `RegionRoute` | `RegionDef.backendEndpoint` in PM dials external hostnames via `ExternalJumpPreset` | Same category as DynamicExternalUpstream; also requires post-resolution validation |
| `DynamicInternalUpstream` | Can only choose existing BackendRef from current route's `backendRefs` | Authorization constraint is decent; final address safety should be uniformly guaranteed by the chosen BackendRef's main backend dial path |
| `DirectEndpoint` | The request supplies an IP, but it must exist in the Endpoint/EndpointSlice corresponding to the current route's `backendRefs` | Endpoint membership only proves "belongs to an authorized Service"; does not automatically prove address safety; the final direct-dial address should also be validated |
| `RequestMirror` | Mirror backendRef is configured; independent of main request success/failure | After LB selects an endpoint, validation should be the same as the main backend; falling back to unchecked K8s DNS + reqwest's own resolution when endpoint store is missing is not allowed |
| `AllEndpointStatus` | Request triggers fan-out to all endpoints of the current route's backend | Although not the main upstream, it is a data-plane endpoint dial; before fan-out, unsafe endpoints must be filtered out and a bounded plugin log written |
| Endpoint / EndpointSlice LB | Operator/K8s provides endpoints; the main request dials directly | LB-selected addresses must be validated near `HttpPeer` creation; production rejects loopback + link-local by default |

## Current Compatibility Constraints

Edgion's local integration fixtures rely on `127.0.0.1` Endpoint/EndpointSlice, so test/dev compatibility must be explicitly isolated and must not affect production default policy. Currently recommended layering:

- External / DNS jump: by default reject loopback + link-local; operator override may only permit loopback.
- Endpoint/EndpointSlice-derived dial: production by default rejects loopback + link-local; `--integration-testing-mode` may permit loopback to support local fixtures, but still rejects link-local / metadata; the operator override likewise may only permit loopback.

## Relationship with the ForwardAuth / Webhook Rule

[forward-auth-webhook-ssrf.md](../regression-guards/forward-auth-webhook-ssrf.md) closes findings of the form "static operator URI should ban internal access", because ForwardAuth/Webhook's legitimate use case is precisely to access internal services, and a blanket private-network block would break the feature.

This rule is narrower: it does not ban internal networks, only loopback and link-local / metadata; and it focuses on the consistency of safety policies among Gateway upstream dial paths.

## Integration evidence: external-loopback rejection (ARCH-354)

The DynamicExternalUpstream suite owns `deu-egc` with explicit
`allowLoopbackUpstream: false`, reached through its private GatewayClass and
Gateway listener on port 32005 in both local and Kubernetes fixtures. Its three
routes use that listener. The shared Kubernetes EGC legitimately permits loopback
and must not be changed to satisfy this suite.

The two matched-localhost rejection probes require both HTTP 500/502 and the
exact fresh request's `BackendLocalhostNotAllowed` error diagnostic in the
canonical access log. Plugin selection happens before address admission, so
`upstream selected`, a generic 5xx, or a failed localhost dial does not establish
rejection. Missing, malformed, wrong-request or unavailable evidence fails.
The suite reuses canonical traceparent conversion and authenticated AccessLogClient
retry; bounded marker-specific cleanup preserves unrelated request evidence.

Source and focused regression coverage live in
`edgion-tests/src/client/suites/edgion_plugins/dynamic_external_upstream/dynamic_external_upstream.rs`.
Pure verdict tests distinguish genuine denial evidence from ordinary dial failure,
selection-only logs, stale/missing trace IDs, and wrong/malformed diagnostics.
They do not execute an admission bypass or prove live Kubernetes publication.
Re-evaluate this boundary if the diagnostic schema, listener-policy ownership,
fixture attachments, or terminal logging lifecycle changes.
