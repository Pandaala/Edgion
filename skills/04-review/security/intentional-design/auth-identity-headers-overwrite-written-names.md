---
name: auth-identity-headers-overwrite-written-names
description: Use when reviewing auth identity-header spoofing, leftover X-Consumer-Username / X-Anonymous-Consumer after success, multi-auth plugin header order, or proposals to unify sanitization across BasicAuth, KeyAuth, JwtAuth, HmacAuth, HeaderCertAuth, LDAP, and ForwardAuth.
status: accepted-tradeoff
finding_id: D0037
closed: 2026-09-05
knowledge_type: intentional-design
---

# Auth Identity Headers Overwrite Written Names Only — ACCEPTED-TRADEOFF

## Conclusion

Authentication plugins retain per-plugin output ownership. Edgion does not reserve a
global `X-Consumer-*` family, does not strip complementary Kong-style identity names on every
success path, and does not define a multi-plugin sanitization order.

This closes D0037 and D0038 as written. It never accepted client-controlled values on
explicitly configured claim destinations. ISSUE-002 now clears all configured JwtAuth /
OpenID Connect claim and JweDecrypt payload projection destinations before adding bounded,
trusted values, including the executing plugins' no-claim successful continuations.
See the [projection guard](../regression-guards/auth-claim-header-projection.md).
ISSUE-003 likewise clears HeaderCertAuth's active certificate identity destinations;
missing SANs and anonymous continuations cannot retain client input there. See the
[certificate projection guard](../../header-cert-auth/regression-guards/certificate-identity-header-projection.md).
D0052 historically fixed ordering and collisions without changing skip-vs-strip behavior.

Fix suggestions of the form "unify identity-header sanitization across every auth plugin",
"always strip client-supplied `X-Consumer-Username` / `X-Anonymous-Consumer` before auth",
"make last-plugin or first-plugin identity ownership canonical", or "add forged-header
upstream-observation coverage that assumes that contract" are **not accepted**.

## Core Rationale

**1. Same-name mutation is replace when the plugin writes, matching Envoy and Kong defaults.**

`PluginSession::set_request_header` routes through `insert_header` and replaces every field
line for that name. Envoy `jwt_authn` `claim_to_headers` replaces an existing header of the
same name when it writes a claim. Envoy `ext_authz` `allowed_upstream_headers` overrides;
append is a separate `allowed_upstream_headers_to_append` opt-in. Kong
`kong.service.request.set_header` overrides; `add_header` is the multi-value opt-in.

Envoy additionally strips every configured `claim_to_headers` / `forward_payload_header`
name before applying rules, including when the claim is absent. Edgion's
`set_claims_headers` now clears its complete configured plan before projection (ISSUE-002).
Disabled or condition-skipped Edgion plugins remain inert, unlike Envoy's filter-wide cleanup
before bypass. Do not cite this tradeoff as full Envoy sanitization parity.

**2. Multi-plugin stacking is last-write-wins, not a missing reserved-name contract.**

Routes rarely run two auth plugins. When they do, later `set_request_header` calls overwrite
the same name. Some output names are hardcoded (`X-Consumer-Username` on BasicAuth success
and several anonymous paths; `X-Anonymous-Consumer` on anonymous allowance), so stacked
plugins cannot always isolate identity with custom names. That leftover sharing is accepted
as last-write-wins, not a reason to invent a shared sanitizer.

**3. Unwritten default consumer names stay per-plugin output, not Kong's global family.**

Kong's `set_consumer` set-or-clears the whole `X-Consumer-*` / `X-Anonymous-Consumer`
family. LDAP already clears its configured anonymous marker on success; other plugins do
not. JwtAuth writes consumer headers only when `claimsToHeaders` or `anonymous` says so.
A filter-wide strip of the default Kong names would make those names reserved even when no
plugin writes them.

## Fix Suggestions Not Accepted

- Cross-plugin identity-header sanitization helper — last-write-wins plus whatever names
  each plugin actually writes already cover stacked auth.
- Filter-wide strip of default `X-Consumer-Username` / `X-Anonymous-Consumer` on every
  auth plugin — JwtAuth's default is "no consumer header". A global strip would reserve
  names the plugin does not own.
- Multi-plugin deterministic identity ownership beyond ordinary overwrite — encoding
  first-wins, or forcing complementary clears on every success path, is a product feature.
- D0038-style forged-header integration scenarios that assume a unified ownership
  contract — those scenarios would encode the rejected D0037 contract.

## Re-evaluation Triggers

Re-open this decision only if:

- An upstream authorization path treats leftover `X-Anonymous-Consumer` as authoritative
  even after a successful authenticated username write.
- Edgion documents `X-Consumer-*` as a reserved gateway-owned family (Kong-style) rather
  than per-plugin output names.
- A supported multi-auth composition is added that requires complementary clears because
  last-write-wins of hardcoded names is no longer acceptable.

Do not infer sanitize-before-rules behavior from the D0052 projection-order correction.

## Reference Cases

- D0037 / D0038 closed as accepted tradeoff (campaign files deleted)
- `edgion-gateway/src/plugins/runtime/session_adapter.rs` (`set_request_header` → `insert_header`)
- `edgion-gateway/src/plugins/http/common/auth_common.rs` (configured-destination cleanup and projection)
- `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` (`apply_authenticated_headers` clears the configured anonymous marker)
- `docs/en/user-guide/http-route/filters/edgion-plugins/jwt-auth.md` (no default consumer header)
- [D0052 deterministic configured claim projection](../regression-guards/auth-claim-header-projection.md)
- Envoy `jwt_authn` replace-on-write plus configured-name sanitize-before-rules
- Kong `set_consumer` set-or-clear of the consumer / anonymous header family

## Resolution references

- `D0037` — 2026-09-05 (accepted tradeoff).
- `D0038` — 2026-09-05 (accepted tradeoff).
- `D0052` — 2026-09-11 (deterministic projection fixed; skip-write behavior unchanged).
