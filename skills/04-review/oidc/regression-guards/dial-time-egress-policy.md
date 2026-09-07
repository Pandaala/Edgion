---
name: oidc-dial-time-egress-policy
description: Use when reviewing OpenID Connect SSRF or DNS findings that discovery, JWKS, token, or other IdP endpoints resolve without EgressResolver, or when a fix proposes a plugin-local blockPrivate field, URL rewriting, or leaving the SAN-pinning client on the system resolver.
status: fixed
finding_id: D0056
closed: 2026-09-04
knowledge_type: regression-guards
---

# OpenID Connect Dial-time Egress Policy — FIXED

## Conclusion

`openid_connect` must classify every outbound IdP destination at dial time through
the shared Gateway `EgressResolver`, disable proxy-environment pickup, and keep
denying redirects. Both the default factory client and the SAN-pinning
`use_preconfigured_tls` client share that transport contract. RFC1918 private
destinations follow gateway `default_block_private()` so in-cluster identity
providers stay reachable unless the operator enables the gateway switch.

Fix suggestions of the form "add `blockPrivate` on OpenidConnectConfig",
"rewrite IdP URLs to resolved IPs", "preflight DNS at plugin construction",
"install the resolver only on the default client", and "honor HTTP(S)_PROXY"
are **not accepted**.

## Core Rationale

**1. One client covers configured and discovered endpoints**

Discovery, JWKS, token, introspection, userinfo, and revocation all use the
per-instance `reqwest::Client`. Installing `EgressResolver` as that client's
DNS resolver classifies hostname destinations at dial time, including
endpoints learned later from the discovery document.

reqwest skips custom DNS for IP-literal hosts, so `admit_oidc_outbound_url`
classifies those URLs with the same `filter_resolved_addrs` policy before
send. The URL is not rewritten.

**2. The original hostname remains the TLS identity**

Reqwest still presents the URL hostname for SNI, certificate verification,
custom CA, and SAN pinning while connecting only to admitted resolved
addresses. Rewriting the URL to an IP would break hostname verification.

**3. Gateway defaults are the operator-visible policy**

Loopback, link-local (including IMDS `169.254.169.254`), and unspecified
addresses are always rejected. Private destinations inherit
`outbound_egress_block_private` / `EDGION_OUTBOUND_BLOCK_PRIVATE` (default
`false`), matching `ldap_auth`. A plugin-local schema field would duplicate
that switch and silently break existing private IdPs if defaulted to `true`.

**4. SAN pinning is a TLS exemption, not an egress exemption**

The SAN branch still attaches a custom `ServerCertVerifier` through
`use_preconfigured_tls` because the factory TLS helpers cannot. Proxy, redirect,
timeout, and resolver knobs go through `apply_outbound_policy` so the two
branches cannot drift.

## Fix Suggestions Not Accepted

- Plugin-level `blockPrivate` — duplicates the gateway egress default
- Rewriting IdP URLs to IPs — breaks TLS/SAN hostname verification
- Controller or construction-time DNS with a later hostname connect — TOCTOU / DNS rebinding
- Resolver on the default branch only — the SAN client would remain an escape hatch
- Changing existing OpenID Connect integration tests — tracked separately as D0057

## Re-evaluation Triggers

Re-open this decision only if:

- OpenID Connect gains a Kubernetes Service `backendRef` target that should follow
  the EndpointSlice path instead of hostname egress
- Gateway egress defaults change so that typical in-cluster IdPs would be denied
  without a plugin-local override
- Reqwest starts invoking `dns_resolver2` for IP-literal hosts, making the
  pre-send literal guard redundant

## Reference Cases

- D0056 — `tasks/pending/edgion-gateway-file-optimization/deferred/D0056-openid-connect-dial-time-egress-policy.md`
- Source: `edgion-gateway/src/plugins/http/openid_connect/plugin.rs`
- Sibling: `skills/04-review/ldap-auth/regression-guards/dial-time-egress-policy.md`

## Resolution references

- `D0056` — 2026-09-04.
