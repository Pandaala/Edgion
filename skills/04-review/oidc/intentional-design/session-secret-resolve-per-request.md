---
name: session-secret-resolve-per-request
description: OIDC secrets and session ciphers are prepared once per plugin generation; preserves the historical F-015 global-store analysis without treating its former per-request fallback as current behavior.
knowledge_type: intentional-design
---

# OIDC secret preparation and global-store fallback

## Current conclusion

`OpenidConnect` prepares credentials during plugin construction. In
`edgion-gateway/src/plugins/http/openid_connect/plugin.rs`, construction calls
`prepare_secrets` from `session_cookie.rs`. That helper consumes the non-empty
`resolved_oidc_client_secret` / `resolved_session_secret` carriers, falling back
through `resolve_secret_once` to `get_secret` only when resolved material is
absent or empty. It prepares the session cipher once and stores both outcomes
in `PreparedOidcSecrets` as ready or unavailable.

Request-time `client_secret()` and `session_cipher()` borrow those prepared
values. They perform no global Secret-store lookup, key derivation, or new
Secret copy. A missing or unusable credential remains unavailable for that
plugin generation and fails closed when required. Recovery uses a newly
constructed generation after configuration resync; changing the fallback store
alone does not mutate an already-published generation.

A second lazy `OnceLock` cache or a request-time retry of Secret resolution
would not remove an existing hot-path cost. It would introduce another
credential lifecycle alongside the immutable prepared generation.

## Store ownership

The production Gateway still has no writer to `GLOBAL_SECRET_STORE` in
`edgion-common/src/secret/secret_store.rs`. Controller Secret processing writes
to its separate `ControllerRuntime::secret_store`, including authoritative
initialization and deletion. Thus the Gateway fallback normally reads an empty
store; it does not contend with Controller Secret updates or deep-copy their
contents. Tests can populate the global store to exercise fallback behavior;
that is not a production writer.

## Historical F-015 decision and its limit

F-015 (Round B, closed 2026-05-27) reviewed the former
`resolve_session_secret` / `resolve_client_secret` implementation. Its happy
path used controller-resolved values; its request-time degraded fallback read
the empty global store. The claimed per-request Secret deep-copy and contention
with Controller TLS consumers were therefore unsupported.

The old page also argued that resolving the degraded path per request was
necessary and rejected preparation-time caching. That reasoning is obsolete:
the current implementation explicitly prepares credentials and unavailable
outcomes once per generation. Do not use the old argument to restore
request-time lookups or suppress defects in preparation or resync recovery.
The independent-store ownership evidence above remains applicable.

## Verification and re-evaluation

Current source evidence:

- `edgion-gateway/src/plugins/http/openid_connect/plugin.rs`: construction and
  `PreparedOidcSecrets` ownership.
- `edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs`:
  `resolve_secret_once`, `prepare_secrets`, `client_secret`, `session_cipher`.
- `edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test.rs`:
  prepared carrier cleanup and generation/fallback rotation coverage.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/secret.rs`:
  writes to the runtime-owned store.

These are source-review references, not a fresh claim that the Gateway tests
were executed. Re-evaluate if a production Gateway store writer is added,
Secret resolution returns to the request path, or configuration resync stops
constructing a new prepared generation. Preserve fail-closed credential behavior
and verify generation isolation before adding any cache or refresh mechanism.
