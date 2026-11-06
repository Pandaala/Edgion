# res3-04: OpenID Connect accepts `http://` discovery and never pins the scheme or issuer of discovered endpoints

- **Category**: best-practice
- **Severity**: P3 (security; downgraded from P2 by independent verification)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs:410-415` (`discovery` may be `http://`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs:147-148` (`introspection_endpoint`: no validation)
  - `edgion-gateway/src/plugins/http/openid_connect/plugin.rs:79-96` (`admit_oidc_outbound_url`: IP-literal egress check only)
  - `edgion-gateway/src/plugins/http/openid_connect/plugin.rs:156-186` (`build_discovery_validator`: presence checks only)
  - `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs:280-305,365-370`

## Summary
Validation lets the discovery URL be `http://`. The plugin then fetches the discovery document and
uses its `jwks_uri`, `token_endpoint`, `userinfo_endpoint` and `introspection_endpoint` without
checking their scheme. It also does not check that the document's `issuer` matches the discovery
URL. If the discovery URL is cleartext, or a TLS-protected discovery document lists any `http://`
endpoint, a network attacker can:
- substitute the JWKS and so mint tokens the gateway accepts;
- read the `client_secret` sent to the token or introspection endpoint;
- tamper with introspection answers.
The configured introspection override is also unchecked.

## Current behavior (evidence)
- `OpenidConnectConfig::get_validation_error` (`openid_connect.rs:413-415`) only requires the
  `discovery` string to start with `https://` or `http://`. `introspection_endpoint:
  Option<String>` (`:147-148`) has no validation at all.
- `JwksProvider` fetch (`jwks_provider.rs:280-305`) rejects an empty `issuer`/`jwks_uri`, then runs
  the config validator.
- `build_discovery_validator` (`plugin.rs:156-186`) only checks that the authorization, token and
  userinfo endpoints are *present*.
- Before each fetch, `admit_oidc_outbound_url` (`plugin.rs:79-96`) only applies the
  private-address egress policy to IP-literal hosts. The JWKS fetch at `jwks_provider.rs:368-370`
  goes to whatever `discovery.jwks_uri()` says, including `http://`.
- No code compares `doc.issuer()` with the discovery URL prefix, so the cross-issuer mix-up check is
  absent.

## Industry comparison
- OpenID Connect Discovery 1.0 §3 says `issuer` and `jwks_uri` **MUST** use the `https` scheme.
  §4.3 says the returned `issuer` **MUST** be identical to the issuer URL used to build the
  discovery URL, or the document must not be used.
- RFC 8414 (OAuth AS metadata) §3.3 has the same issuer-identity requirement.
- Kong `openid-connect` and the `lua-resty-openidc` library validate the discovered issuer.
  Envoy `jwt_authn` takes an explicit `issuer` plus `remote_jwks.http_uri`, so a discovery document
  cannot redirect key retrieval.

## Impact
The JWKS is the trust root for every `JwksOnly`/`Auto` verification. A cleartext or redirectable
JWKS is a full authentication bypass for anyone on the path between the Gateway and the IdP. That
path is often the public internet for SaaS IdPs. Sending the client secret to an `http://` token
endpoint discloses it.

## Suggested direction
1. Admission (Controller and Gateway, `get_validation_error`): require `https://` for `discovery`
   and for an explicit `introspectionEndpoint`. Add one explicit, documented opt-in such as
   `allowInsecureHttp: true` for lab IdPs. Without that flag, reject `http://`.
2. Discovery validation (`build_discovery_validator`): reject the document if `issuer` ≠ the
   configured issuer (derived from the discovery URL minus `/.well-known/openid-configuration`, or
   `issuers[0]` if configured). Also reject it if any endpoint the plugin will use is not `https://`,
   unless the same opt-in is set. Reuse `OidcDependencyFailure::InvalidDocument`.
3. Keep the existing egress IP check. It covers a different threat (SSRF to private ranges).

## Notes
- Reviewed `skills/04-review/oidc/*`. `dial-time-egress-policy.md` covers private-address egress.
  `redirect-uri-*` pages cover *downstream* redirect URI scheme/host derivation.
  `outbound-tls-policy-construction-fails-closed.md` covers TLS policy construction once TLS is used.
  None of them accepts cleartext discovery or unpinned discovered endpoints, so this is not a
  documented tradeoff.
- `tasks/improve-issues/gw-plugin-03-oidc-access-token-only-in-replica-memory.md` is unrelated
  (session storage).

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P2 -> P3.
- `get_validation_error` accepts `http://` discovery (`openid_connect.rs:413-415`); `introspection_endpoint` has no validation.
- `build_discovery_validator` (`edgion-gateway/src/plugins/http/openid_connect/plugin.rs:156-186`) checks endpoint presence only; `admit_oidc_outbound_url` (`:79-96`) is an IP-literal egress check. No issuer-vs-discovery-URL comparison exists (`grep issuer` hits only token claim validation in `token_verify.rs`/`config_validation.rs`).
- Downgrade reason: exploitation requires the operator to configure a cleartext discovery URL or the IdP itself to publish `http://` endpoints; the TLS-protected default path is safe. The industry comparison is overstated: Kong/APISIX (lua-resty-openidc) also accept `http://` discovery and do not compare the discovered issuer with the discovery URL. The spec-compliance hardening (https-only unless opt-in, issuer identity check) is still worth doing.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `openid_connect.rs` (resources) changed only claim-header projection compilation; `get_validation_error` still accepts `http://` discovery (`:413-414`) and `introspection_endpoint` (`:148`) is unvalidated.
- Gateway OIDC diffs add an absolute session deadline and claim-header projection changes; `admit_oidc_outbound_url` (`plugin.rs:77`) and `build_discovery_validator` (`plugin.rs:156`) are unchanged, and no added line compares the discovered issuer or endpoint schemes. `jwks_provider.rs` is unchanged.
