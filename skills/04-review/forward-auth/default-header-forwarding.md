---
name: default-header-forwarding
description: Use when reviewing forward_auth header-forwarding findings; documents why forwarding all non-hop-by-hop headers by default (via request.headers.forwardAll = true) is an accepted industry-standard tradeoff and not a vulnerability.
---

# ForwardAuth Default Forwards All Request Headers: Design Tradeoff, Not Vulnerability

## Conclusion

`ForwardAuthConfig::default()` sets `request.headers.forward_all = true`, meaning the auth service receives **all** non-hop-by-hop headers (including `Cookie`, `Authorization`) unless the operator overrides with `request.headers.forwardAll: false`. This is an **already-evaluated design tradeoff**, consistent with industry-standard gateway defaults. It **should not** require changing the default, adding an allowlist, or forcing the operator to set selective forwarding.

## Reasons

1. **Aligns with industry-standard gateway defaults**:
   - Traefik `forwardauth` — forwards all by default
   - Envoy `ext_authz` — forwards all by default
   - Edgion follows these two mainstream implementations' defaults
   - APISIX `forward-auth` is the minority view (forwards only listed headers by default); diverging from it would cost more than it benefits

2. **Target is Controller/CRD-managed, not user-controlled**: ForwardAuth's `target` field comes from Controller-managed CRD config, and may be either a Kubernetes Service reference (`target: { name, namespace, port }`, resolved via EndpointSlice at runtime) or an origin-only external URL (`target: { url, blockPrivate }`). Either way it is set by a cluster admin via the CRD (RBAC-gated), not by a regular request-time user — there is no scenario where a regular user can induce forwarding to an arbitrary address. The Service form gets an additional runtime SSRF guard (`validate_endpoint_backend_addr`); the URL form carries its own dedicated SSRF egress guard (loopback/link-local/unspecified always blocked, `blockPrivate: true` also blocks RFC1918/unique-local).

3. **Documentation explains the choice clearly**:
   - `docs/{zh-CN,en}/user-guide/edgion-plugins/forward-auth.md` has a dedicated `request.headers.forwardAll Default Semantics` section explaining the three forwarding modes (`forwardAll: true`, `forwardAll: false` + `origin`, or mixed)
   - The FAQ compares forward-all vs selective forwarding, explicitly recommending selective for "external auth services, principle of least privilege"
   - `Cookie` forwarding behavior is called out explicitly

4. **Cost of changing the default**: changing the default to `forwardAll: false` would break all existing deployments that rely on the default transparent-passthrough behavior, and conflict with users' existing mental model from Traefik/Envoy.

## Decision Rule

When a review or audit report contains the following patterns, close as "design tradeoff":

1. "ForwardAuth forwards all request headers (including sensitive credentials) by default"
2. "Recommend changing the default to empty list / explicit allowlist; turn full forwarding into explicit opt-in"
3. "Documentation should warn about this default behavior" (the docs already have a dedicated section)
4. "At runtime should log a warning in full-forwarding mode"

Verify three points before closing:

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/forward_auth.rs`'s `default_forward_auth_request()` still sets `r.headers.forward_all = true`, and `ForwardAuthConfig::default()` also sets `request.headers.forward_all = true` (default not changed to `false`)
- `ForwardAuthConfig` uses `request: OutboundRequestSpec` with `headers: ListFields { forward_all: bool, ... }`; the operator-facing field is `request.headers.forwardAll` in YAML (not the old `requestHeaders: Option<Vec<String>>`)
- `docs/{zh-CN,en}/user-guide/edgion-plugins/forward-auth.md` still contains the `forwardAll` semantics section listing all three forwarding modes

All three satisfied → design tradeoff; close.

## New Schema vs Old Schema

The old schema (`requestHeaders: Option<Vec<String>>`) has been replaced. The field shape is now:

```yaml
request:
  headers:
    forwardAll: true          # default for forward_auth (was: requestHeaders = null → forward all)
    origin:                   # specific headers from incoming request (was: requestHeaders list)
      - { name: Authorization, presence: skipIfMissing }
    custom:                   # injected from config or secret
      - { name: X-Source, value: edgion }
```

The design decision (forward-all is the safe default for auth service transparency) is preserved unchanged. Only the field shape changed.

## Exceptional Scenarios (need reassessment)

- If in the future `request.path.template` is made to accept user-controlled input (breaking the "operator-controlled target" invariant) → the SSRF analysis changes; re-audit
- If industry mainstream defaults change (e.g. Traefik / Envoy upgrade to default allowlist mode) → reassess whether to follow

## Background Notes

- Original decision date: 2026-04-21
- Decision source: `tasks/security-audit/` M7 entry
- Schema refactor: tasks 1–9 of the shared outbound-call core refactor (2026-06)
