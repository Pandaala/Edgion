# ForwardAuth Default Forwards All Request Headers: Design Tradeoff, Not Vulnerability

## Conclusion

The `forward_auth` plugin, when `request_headers = None`, forwards **all** non-hop-by-hop headers (including Cookie, Authorization) to the auth service. This is an **already-evaluated design tradeoff**, consistent with industry-standard gateway defaults. It **should not** require changing the default, adding an allowlist, or forcing the operator to configure `requestHeaders`.

## Reasons

1. **Aligns with industry-standard gateway defaults**:
   - Traefik `forwardauth` — forwards all by default
   - Envoy `ext_authz` — forwards all by default
   - Edgion follows these two mainstream implementations' defaults
   - APISIX `forward-auth` is the minority view (forwards only listed headers by default); diverging from it would cost more than it benefits
2. **URI is statically configured by the operator**: ForwardAuth's `uri` field comes from Controller-managed CRD config, not user input; there is no scenario where "users can induce forwarding to a malicious address". The INFO line in audit 01-auth-plugins.md states clearly: "URI statically configured by admin; no user-controlled SSRF surface"
3. **Documentation already explains clearly**:
   - `docs/zh-CN/user-guide/edgion-plugins/forward-auth.md` has a dedicated "### requestHeaders behavior" section, listing the three behaviors `null` / `[]` / specific list
   - The FAQ section compares "forward-all vs selective forwarding" use cases, explicitly recommending selective for "external auth services, principle of least privilege"
   - Cookie behavior has its own callout
4. **Cost of changing the default**: changing the default to empty list / allowlist would break all existing deployments that rely on the default behavior, and conflict with users' existing mental model from Traefik/Envoy

## Decision Rule

When a review/audit report contains the following patterns, close as "design tradeoff":

1. "ForwardAuth forwards all request headers (including sensitive credentials) by default"
2. "Recommend changing the default to empty list / explicit allowlist; turn full forwarding into explicit opt-in"
3. "Documentation should warn about this default behavior" (the documentation already has a dedicated section; further reinforcement would be noise)
4. "At runtime should warn! to record full-forwarding mode"

Verify three points before closing:

- `forward_auth/plugin.rs`'s `build_auth_headers` still keeps the "`None` → `forward_all_headers`" branch (default not changed to allowlist)
- `BasicAuthConfig`/`ForwardAuthConfig.request_headers` is still `Option<Vec<String>>` and defaults to `None`
- `docs/{zh-CN,en}/user-guide/edgion-plugins/forward-auth.md` still contains the "### requestHeaders behavior" section listing the three configurations' explicit behaviors

All three satisfied → design tradeoff; point to `tasks/security-audit/01-auth-plugins.md` M7 NOT-AN-ISSUE decision and close.

## Exceptional Scenarios (need reassessment)

- If in the future the ForwardAuth `uri` field is changed to support template variables (e.g., `${request.header.x-target}`) making it user-controllable input → this conclusion no longer holds; re-audit
- If industry mainstream defaults change (e.g., Traefik / Envoy upgrade to default allowlist mode) → reassess whether to follow

## Background Notes

- Decision date: 2026-04-21
- Decision source: `tasks/security-audit/` M7 entry
- Comparison files:
  - `auth-timing/basic-auth-username-enumeration.md` (M4, doc-only close, has actual exploitability but code change costs more)
  - `auth-timing/key-auth-hashmap-not-ct.md` (M10, NOT-AN-ISSUE, no actual exploitability)
  - This file (M7, NOT-AN-ISSUE, has exploitability but is an operator config-layer tradeoff, not a code default oversight)
