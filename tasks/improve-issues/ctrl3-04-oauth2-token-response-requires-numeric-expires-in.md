# ctrl3-04: OAuth2 credential source rejects RFC-conformant token responses without a numeric `expires_in`

- **Category**: design-error
- **Severity**: P3
- **Scope**: local
- **Location**:
  - `edgion-controller/src/services/credential_manager/providers/oauth2.rs:376-381` (`OAuthSuccessResponse { expires_in: u64 }` required)
  - `edgion-controller/src/services/credential_manager/providers/oauth2.rs:346-365` (`parse_success_response`: any decode failure → `permanent()`)
  - `edgion-controller/src/services/credential_manager/service.rs:1351-1356` (`Permanent` → `ReconcileFailure::rotation()`), `:551-564` (retry only at `rotation.interval`, default 900 s)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/credential_injector.rs:256-277` (no material → `MissingKeys`, injector unresolved)

## Summary

The OAuth2 client-credentials provider deserializes the token response into a struct whose
`expires_in` is a mandatory `u64`. RFC 6749 §5.1 makes `expires_in` only RECOMMENDED, and some
widely deployed token endpoints omit it or encode it as a JSON string. Such a response is
classified as a *permanent* failure, retried only every rotation interval, and every consuming
`CredentialInjector` stays unresolved (fail-closed), with no configuration knob to supply a
default lifetime.

## Current behavior (evidence)

- `OAuthSuccessResponse` requires `access_token: String`, `token_type: String`,
  `expires_in: u64` (`oauth2.rs:376-381`). `serde_json::from_slice` fails when the field is absent
  or a string such as `"3599"`, and the error maps to `OAuth2ProviderError::permanent()`
  (`:347`).
- `provider_failure` maps `Permanent` to `ReconcileFailure::rotation()` (`service.rs:1354`), whose
  schedule is the full `rotation.interval` (`:562`), so the source sits in
  `CredentialSourceState::Unavailable` and consumers resolve `MissingKeys`
  (`credential_injector.rs:261-265`).
- The CRD exposes no `defaultExpiresIn`-style field (`edgion-resources/src/resources/link_sys/credential_source.rs`
  has no lifetime option), and nothing in `skills/` or `docs/en` documents the requirement.

## Industry comparison

- Envoy's OAuth2 filter has `default_expires_in` for responses without `expires_in`.
- oauth2-proxy, Spring Security and the `oauth2` Rust crate accept a missing `expires_in` (token
  treated as non-expiring or given a configured default) and tolerate numeric strings.
- Known producers of the non-integer form include the Microsoft identity platform v1.0 token
  endpoint (string-encoded `expires_in`); several providers omit the field for client-credentials
  grants.

## Impact

Any LinkSys `credentialSource` pointing at such a provider never produces a token; routes using
the injector fail closed, and the operator only sees a generic "Permanent" failure with a
15-minute retry, which looks like a provider outage rather than a parser restriction.

## Suggested direction

- Accept `expires_in` as either a JSON number or a decimal string; when absent, use a configurable
  `defaultTokenLifetime` (with a conservative default, e.g. the rotation interval) bounded by the
  existing min/max.
- Distinguish "response parse/shape error" from other permanent failures in the source status
  reason so operators can tell interop problems from credential problems.

## Notes

`ctrl-svc-07` questions the whole Controller-side token acquisition design (control-plane
coupling); this item is independent and applies to whichever component ends up calling the token
endpoint.

## Verification (independent)

- **Verdict**: CONFIRMED (P3)
- `providers/oauth2.rs:376-381`: `OAuthSuccessResponse.expires_in: u64` is mandatory; `parse_success_response` (`:346-347`) maps any decode failure (missing field or string `"3599"`) to `OAuth2ProviderError::permanent()`.
- `service.rs:1351-1355`: `Permanent` -> `ReconcileFailure::rotation()`; `service.rs:562` schedules the retry at the full `rotation.interval` (default 900 s), not the transient backoff.
- RFC 6749 section 5.1 marks `expires_in` RECOMMENDED; no knob or documentation in `skills/` or `docs/en` mentions the requirement (grep for `expires_in` finds nothing).
- Interop-only impact (fail-closed, not a bypass), so P3 is right.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `services/credential_manager/` is unchanged on this branch; `providers/oauth2.rs:376-381` still declares `expires_in: u64` as mandatory.
- `credential_source.rs` still has no default-lifetime knob.
