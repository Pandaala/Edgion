---
name: active-health-check-transport-is-independent
description: Use when reviewing active health-check protocol selection, BackendTLSPolicy inheritance proposals, or encrypted probe design.
knowledge_type: intentional-design
---

# Active health-check transport is independent of BackendTLSPolicy

## Conclusion and scope

`EdgionBackendTrafficPolicy.spec.healthCheck.active` owns active probe protocol,
port, and probe security configuration. The Gateway must not infer or
alter health-check transport from a `BackendTLSPolicy` attached to the same
Service. It must not inherit that policy's trust, SNI, SAN, client identity, or
generation lifecycle.

The `HealthCheckType` values retain literal meanings: `http` and `grpc`
are plaintext, `tcp` verifies only that a socket can be connected, and explicit
`https`/`grpcs` use health-local verified TLS. This
decision covers the Service active-health subsystem in
`edgion-gateway/src/backends/health/check/`; it does not change ordinary
HTTPRoute/GRPCRoute upstream TLS, AllEndpointStatus, LinkSys health checks, or
other outbound clients.

## Rationale and evidence

The product owner explicitly decided on 2026-09-11 that active health checks
are independent and must have no relationship with `BackendTLSPolicy`. This is
the acceptance basis for the separation and its deliberate cost: encrypted
probes need their own TLS configuration instead of reusing an already resolved
request-path policy.

Health checks often intentionally use a different protocol or port from
application traffic. AWS Network Load Balancer exposes TCP, HTTP, and HTTPS as
independent health-check protocols and defaults to TCP. Kong exposes
`tcp`, `http`, `https`, `grpc`, and `grpcs` directly in its active-health schema,
with health-check-local HTTPS SNI and certificate verification settings. Traefik
lets a health check replace the server URL scheme and separately choose gRPC
mode.

Envoy separates application checker type (HTTP, TCP, gRPC, or custom) from its
transport socket. Its default follows the cluster transport socket, but
health-check-specific transport-socket matching and TLS options can override
that behavior. Edgion adopts the useful separation of checker and transport,
not Envoy's cluster inheritance: `BackendTLSPolicy` is route/backend traffic
policy rather than the health check's owning configuration.

Current source matches this decision:

- `ActiveHealthCheckConfig` contains the explicit `HealthCheckType` and probe
  port.
- `probe_http_with_client` and `probe_grpc` construct `http://` targets.
- `probe_tcp` performs only `TcpStream::connect`.
- `HealthCheckConfigStore` is written and reconciled from the effective
  `EdgionBackendTrafficPolicy`; `BackendTLSPolicyStore` is not an input.

Primary references:

- [Envoy health-check API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/core/v3/health_check.proto)
- [Kong health checks](https://developer.konghq.com/gateway/traffic-control/health-checks-circuit-breakers/)
- [Kong upstream schema](https://github.com/Kong/kong/blob/master/kong/db/schema/entities/upstreams.lua)
- [AWS Network Load Balancer health checks](https://docs.aws.amazon.com/elasticloadbalancing/latest/network/target-group-health-checks.html)
- [Traefik service health checks](https://doc.traefik.io/traefik/reference/routing-configuration/http/load-balancing/service/#health-check)

## Alternatives and limits

Automatically inheriting `BackendTLSPolicy` was rejected. It would let an
unrelated request-path policy silently change an existing probe from plaintext
to TLS, couple probe task reloads to another resource generation, and make a
deliberately separate health endpoint harder to express. Copying only the
policy hostname or switching `http://` to `https://` was also rejected because
it would omit trust, identity, validation, and client reuse semantics.

The encrypted extension adds explicit `https` and `grpcs` probe types.
They share health-check-local `OutboundTlsLocal` and resolve it through the
canonical outbound-TLS implementation with explicit defaults. Global outbound
trust/identity is excluded as well. Both protocols reuse the shared verified
outbound TLS builder, including additional SAN and mTLS enforcement. Missing
or invalid local TLS material fails the encrypted probe, never downgrades it.
TLS errors remain separate from policy attachment eligibility, so an invalid
transport cannot silently withdraw the check or promote a competing policy.

Complete health-config equality includes resolved Secret material. Every task
owns its TLS generation and every endpoint exchange owns its client/channel.
A synchronous publication/poll gate prevents admitting old-generation work after
health-config replacement; already submitted transport I/O is in flight and
cancelled best effort. The Coordinator rejects stale health completions.

A standalone `tls` handshake checker is not currently recommended: it gives
more evidence than TCP connect but less than an application probe, and neither
Kong nor AWS NLB exposes it as an active-health protocol. Envoy can effectively
compose a TCP checker with a TLS transport socket, so this can be reconsidered
if Edgion gains a concrete non-HTTP TLS backend use case. Envoy custom checkers,
Kong-style separate healthy/unhealthy intervals, request headers, and other
tuning are useful precedent but are not part of the first protocol increment.

## Verification and re-evaluation

The explicit product decision and linked primary documentation were reconciled
on 2026-09-11. The original documentation-only change established plaintext probe
independence; the encrypted extension preserves that ownership boundary. Focused
resource, Controller, Gateway, structural-CRD, dynamic-registry, and live
TLS/mTLS HTTP/gRPC integration coverage passed. Independent design and final
code reviews closed the publication race, strict CA parsing, and GRPCS origin
findings. The current resource regression and complete CRD coverage guards were
rerun successfully on 2026-09-13.

Re-evaluate if Edgion adopts a general cluster transport object that explicitly
owns both application and probe transport, if a protocol specification requires
health checks to use the data connection's security context, or if operators
produce a concrete need for handshake-only TLS probes.

## Resolution references

- `D0005` / `active-health-check-tls-contract` — the original task correctly
  identified that encrypted probes were unavailable, but its proposed
  `BackendTLSPolicy` relationship was rejected. The capability gap is retained
  under an independent health-check contract.

## Actual reference and generation ownership

The health snapshot retains the actual winning EBTP Arc; the active config is a
borrowed view. `HealthTlsRefs` binds custom CA ordinals and the client Secret to
that owner's same-namespace fields. Cold encrypted transport construction uses
the canonical outbound material proof before PEM/keypair validation and explicit
empty defaults. Valid bytes with wrong identity or ConfigMap provenance cannot
satisfy a declaration. Source name/raw namespace/UID changes rotate the health
generation even when active configs compare equal. This preserves the existing
per-poll generation fence and does not create request-path validation, BTP
inheritance, or a global TLS policy dependency.

Default-namespace raw source support is confined to these finite bound slots and
ordinary EBTP dependency notifications; it does not grant cross-namespace access
or change frozen credential authority. Re-evaluate if health config publication
again detaches material from its actual declaring policy.
