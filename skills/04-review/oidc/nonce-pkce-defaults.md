---
name: nonce-pkce-defaults
description: Use when reviewing OIDC use_nonce or use_pkce default-off findings; documents why both default to false per spec and why changing the default to true is not accepted.
---

# OpenidConnect `useNonce` / `usePkce` Default Off: Design Tradeoff + Doc Recommendation

## Conclusion

The `openid_connect` plugin's `use_nonce` and `use_pkce` both default to `false`. **This is an already-evaluated design tradeoff** — the OIDC spec defines both as OPTIONAL for the authorization-code flow; mainstream gateway implementations are consistent. It **should not** require changing the default to `true`; the operator should be advised to enable them explicitly via documentation.

## Reasons

1. **Allowed by the OIDC spec**: Core spec § 3.1.2.1 defines `nonce` as OPTIONAL for the authorization-code flow; PKCE (RFC 7636) is a separate recommended mechanism, not a MUST
2. **Gateway industry defaults are consistent**:
   - Kong `oidc` plugin — default off
   - APISIX `openid-connect` — default off
   - Envoy `oauth2` filter — does not enforce
   - oauth2-proxy — recent versions default PKCE on; nonce still off
   - Edgion aligns with mainstream implementation behavior
3. **Major threats are already covered along the Edgion code path**:
   - Uses authorization-code flow + **server-side token exchange** (ID Token does not pass through the browser)
   - `state` verification (`openid_impl.rs`) blocks classic CSRF
   - The roles of nonce / PKCE are defense in depth (IdP compression/replay, multi-client token mixing, public-client scenarios, etc.)
4. **Breaking impact of changing the default**:
   - nonce default on → non-standard / older IdPs may not write `nonce` back into the ID Token's `nonce` claim → callback verification fails → existing deployments lose login
   - PKCE default on → IdPs not supporting PKCE (rare but extant) reject the `/authorize` request

## Decision Rule

When a review/audit report contains the following patterns, close as "design tradeoff + already-existing doc recommendation":

1. "OIDC nonce default off does not protect against ID Token replay injection"
2. "use_nonce / use_pkce default should be true"
3. "Should enforce nonce / PKCE"
4. "Not enabling nonce should warn! at runtime"

Verify three points before closing:

- `use_nonce` / `use_pkce` in `edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs` still default to `false`
- The implementation branches in `edgion-gateway/src/plugins/http/openid_connect/plugin.rs` and `edgion-gateway/src/plugins/http/openid_connect/token_verify.rs` are still correct:
  - When enabled, `/authorize` carries the `nonce` parameter, the state cookie stores `nonce`, and the callback verifies the `nonce` claim
  - When enabled, `/authorize` carries `code_challenge`, and token exchange carries `code_verifier`
- `docs/{zh-CN,en}/user-guide/edgion-plugins/openid-connect.md` still contains the "**Recommended Security Switches**" / "Recommended Security Switches" section, explicitly recommending `useNonce: true` + `usePkce: true` in production with reasons

All three satisfied → design tradeoff with documented recommendation; point to the M8 FIXED (doc-only) decision in `tasks/security-audit/01-auth-plugins.md` and close.

## Exceptional Scenarios (need reassessment)

- Industry default change: if peer gateways (Kong / APISIX) change the default of nonce / PKCE to on → reassess whether to follow
- OIDC spec revision upgrades nonce to MUST (currently OPTIONAL) → must change default
- Edgion switches to implicit flow or hybrid flow (spec requires nonce MUST) → change default
- Edgion supports a "client submits ID Token" mode (bypassing server-side exchange) → ID Token injection becomes a direct attack surface immediately; must change default

## Background Notes

- Decision date: 2026-04-21
- Decision source: `tasks/security-audit/` M8 entry (Option B — doc-only)
- Same modal as M4: has specific mitigation recommendations → doc-only, do not change code defaults
- Difference from M7: M7 is NOT-AN-ISSUE (no code / no documentation work, fully closed); M8 has substantive operator action recommendations
