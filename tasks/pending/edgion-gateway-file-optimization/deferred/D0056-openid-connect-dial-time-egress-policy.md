# D0056 — Enforce OpenID Connect Dial-time Egress Policy

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0090, OpenID Connect audit
- Category: egress-security
- Closed: 2026-09-04
- Canonical record: [`skills/04-review/oidc/regression-guards/dial-time-egress-policy.md`](../../../../skills/04-review/oidc/regression-guards/dial-time-egress-policy.md)

## Evidence
Before the fix, the default OpenID Connect outbound client omitted `EgressResolver` and `disable_proxy`, and the SAN-pinning branch built a raw reqwest client without either control. Discovery and remotely discovered JWKS, token, introspection, userinfo, and revocation endpoints could therefore bypass dial-time classification through DNS or proxy routing.

## Resolution
Both client factories now share `oidc_outbound_client_config`: Gateway `EgressResolver`, `disable_proxy: true`, and redirect deny. IP-literal URLs are classified before send because reqwest skips custom DNS for literals. The SAN branch still attaches a custom verifier through `use_preconfigured_tls` after `apply_outbound_policy`. Private destinations inherit `default_block_private()`. A plugin-local `blockPrivate` field and URL rewrite were rejected.

## Integration-test impact
D0057 remains the standalone cross-process coverage task. Existing frozen OpenID Connect integration files remain unchanged.
