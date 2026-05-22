---
name: forward-auth-webhook-ssrf
description: Decision rule for ForwardAuth / Webhook SSRF findings. Read this file first when reviewing "uri field lacks internal IP filtering, can probe metadata service" findings.
---

# ForwardAuth / Webhook SSRF — NOT-AN-ISSUE

## Conclusion

The lack of internal IP filtering on the ForwardAuth `uri` and Webhook `uri` **does not constitute a fixable security issue**; classified as NOT-AN-ISSUE. Fix suggestions of the form "add a private-IP block list" or "add an SSRF protection library" are not accepted.

## Core Rationale

**1. Code-layer fixes fundamentally conflict with legitimate use cases**

ForwardAuth is designed precisely to call internal auth services. RFC 1918 private addresses (`10/8`, `172.16/12`, `192.168/16`) are normal addresses for in-cluster auth services in Kubernetes and cannot be blocked. `169.254.0.0/16` (cloud IMDS) is a literal IP that could be blocked, but:
- Coverage is very narrow (only blocks low-level direct-IP attacks)
- Cannot defend against DNS rebinding or hostname forms (e.g., `metadata.google.internal`)
- Not cost-effective enough to justify introducing blocking logic

**2. Threat model requires high-privilege precondition**

Exploiting this path requires:
- ForwardAuth: write permission to HTTPRoute or EdgionPlugin CRDs (namespace-admin level)
- Webhook: write permission to LinkSys CRD (cluster-scoped resource, higher privilege)

An attacker with these permissions can already do far more harmful things (reroute traffic, inject backends, read Secrets, etc.). SSRF is not a boundary breakthrough but lateral movement inside an already-broken boundary. RBAC is the correct defense, not code-layer IP filtering.

**3. Industry comparison**

Traefik `forwardAuth`, Envoy `ext_authz`, APISIX `forward-auth` all do not perform URI-layer SSRF filtering and consistently rely on RBAC + network policy.

## The Correct Defense Layers

- K8s NetworkPolicy: limits target IPs/Ports the Gateway Pod can access
- RBAC: controls who can write HTTPRoute, LinkSys CRDs
- Cloud side: enable IMDSv2 (requires PUT Token; direct GET returns 401)

These are operations/deployment-layer controls, not the gateway code-layer's responsibility.

## Fix Suggestions Not Accepted

- Add a private CIDR block list (conflicts with legitimate use cases)
- Introduce `ssrf-guard` or similar library (overkill, and cannot solve DNS rebinding)
- Block `169.254.x.x` separately (coverage too narrow, can be DNS-bypassed, not cost-effective)
- Require `uri` field to be DNS-resolved and validated (breaks the static nature of config validation; introduces startup-time DNS dependency)
