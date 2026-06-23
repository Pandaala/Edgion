---
name: forward-auth-webhook-ssrf
description: Decision rule for ForwardAuth / Webhook SSRF findings. Read this file first when reviewing "target lacks internal IP filtering, can probe metadata service" or "uri field lacks internal IP filtering" findings.
---

# ForwardAuth / Webhook SSRF — NOT-AN-ISSUE (for additional block-list suggestions)

## Conclusion

For the Webhook plugin, the lack of additional internal IP filtering on the `target` backendRef **does not constitute a fixable security issue beyond what is already implemented**; suggestions of the form "add a private-IP block list" or "add an SSRF protection library" are not accepted.

For ForwardAuth specifically: the plugin already has a code-layer SSRF guard via `validate_endpoint_backend_addr`. The NOT-AN-ISSUE verdict applies to proposals to **add more** filtering (e.g. blocking all RFC 1918 ranges), not to the absence of any guard at all.

## Implementation: What the ForwardAuth SSRF Guard Already Does

ForwardAuth uses a `target: backendRef` (Kubernetes Service name + namespace + port), **not** a free-form URI. The guard operates as follows:

1. The gateway resolves the Service to pod endpoints via EndpointSlice.
2. `ManagedOutboundClient::dial_url` calls `extract_socket_addr` on the resolved URL.
3. If the host is a numeric IP address, `validate_endpoint_backend_addr` is called with `allow_loopback = false`.
4. The guard rejects loopback (`127.x.x.x` / `::1`) and link-local (`169.254.x.x`) addresses by returning an error that propagates as `ManagedOutboundDialError::SsrfRejected`.

Source: `edgion-gateway/src/plugins/http/common/outbound/client.rs` — `dial_url` + `extract_socket_addr`.

Cluster-local DNS names (e.g. `auth-svc.production.svc.cluster.local`) cannot be classified as loopback or link-local at dial time, so they bypass the IP-layer guard. This is intentional and correct — blocking DNS names at IP classification time would require a runtime DNS lookup (introducing startup-time DNS dependency and race conditions).

## Core Rationale for NOT Expanding the Block List

**1. Code-layer IP blocking fundamentally conflicts with legitimate use cases**

ForwardAuth is designed to call internal auth services. RFC 1918 private addresses (`10/8`, `172.16/12`, `192.168/16`) are the normal destination for in-cluster auth services in Kubernetes and cannot be blocked. `169.254.0.0/16` (cloud IMDS) is already blocked by `validate_endpoint_backend_addr`. Adding more CIDR blocks:
- Would break legitimate in-cluster auth deployments
- Cannot defend against DNS rebinding or hostname forms (e.g. `metadata.google.internal`)
- Is not cost-effective enough to justify the breaking change

**2. Threat model requires high-privilege precondition**

Exploiting this path requires:
- ForwardAuth: write permission to HTTPRoute or EdgionPlugins CRDs (namespace-admin level)
- Webhook: write permission to LinkSys CRD (cluster-scoped resource, higher privilege)

An attacker with these permissions can already do far more harmful things (reroute traffic, inject backends, read Secrets, etc.). SSRF via ForwardAuth is not a boundary breakthrough but lateral movement inside an already-broken boundary. RBAC is the correct defense; expanding the IP block list is not.

**3. Industry comparison**

Traefik `forwardAuth`, Envoy `ext_authz`, APISIX `forward-auth` all do not perform URI-layer SSRF filtering beyond what network policy provides, and consistently rely on RBAC + network policy.

## The Correct Defense Layers

- **Existing code guard**: `validate_endpoint_backend_addr` blocks loopback and link-local IPs on resolved pod addresses (forward_auth + any future `ManagedOutboundClient` consumer)
- **K8s NetworkPolicy**: limit which IPs/ports the gateway pod may reach
- **RBAC**: control who may write HTTPRoute, EdgionPlugins, and LinkSys CRDs
- **Cloud side**: enable IMDSv2 (requires a PUT token; direct GET returns 401)

These are the correct layered defenses. The gateway code-layer guard handles the loopback/link-local case; the rest is operations/deployment responsibility.

## Fix Suggestions Not Accepted

- Add a private CIDR block list (breaks legitimate in-cluster auth; RFC 1918 addresses are valid pod IPs)
- Introduce `ssrf-guard` or similar library (overkill, cannot solve DNS rebinding)
- Block `169.254.x.x` separately (already blocked by `validate_endpoint_backend_addr`)
- Require `target` to be DNS-resolved and validated at admission time (introduces startup DNS dependency and race conditions)
- Change `target` from a backendRef back to a free-form URI with IP validation (would remove EndpointSlice-based load balancing and the existing SSRF guard)
