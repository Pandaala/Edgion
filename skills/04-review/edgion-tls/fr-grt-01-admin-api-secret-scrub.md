---
name: fr-grt-01-admin-api-secret-scrub
description: Gateway admin API /configclient/EdgionTls now scrubs spec.secret and spec.client_auth.ca_secret before serialization, so resolved TLS/mTLS private keys never reach the HTTP response. Reject re-reports claiming the admin API leaks EdgionTls private keys via serde skip_serializing_if.
---

# fr-grt-01 — admin API EdgionTls serialization scrubs private-key material

**Status:** ✅ fixed (Option B) · 2026-05-29
**Severity:** P1 (Security) — unauthenticated TLS/mTLS private-key disclosure

## Conclusion

The gateway admin API handlers in `src/core/gateway/api/mod.rs` no longer serialize
resolved private-key material for `EdgionTls`. Both list and get paths route through
dedicated helpers — `to_json_vec_edgion_tls` and `find_edgion_tls` — that call
`scrub_edgion_tls_secrets` before `serde_json::to_value`, nulling out:

- `EdgionTlsSpec.secret: Option<Secret>` (resolved leaf cert Secret → `data["tls.key"]`)
- `EdgionTlsSpec.client_auth.ca_secret: Option<Secret>` (resolved mTLS CA Secret)

Both fields carry `skip_serializing_if = "Option::is_none"`, so once set to `None` they
disappear from the JSON entirely. Non-secret fields (`hosts`, `secret_ref`, etc.) survive.

Re-reports of the form "`/configclient/EdgionTls{,/list}` serializes `tls.key` /
private keys", "`EdgionTlsSpec.secret` has only `skip_serializing_if`, not `#[serde(skip)]`,
so the admin API leaks key material", or "mTLS `ca_secret` is echoed in the admin response"
are **not accepted** — none is still true. Verify the `scrub_edgion_tls_secrets` call in
both `to_json_vec_edgion_tls` and `find_edgion_tls` before re-reporting.

## Why `#[serde(skip)]` on the struct was not the fix

`secret` / `ca_secret` are populated by the Controller (resolving `secret_ref` /
`ca_secret_ref` into real Kubernetes Secrets) and are **serialized on purpose for the
controller→gateway sync transport** (see the doc comment on `ClientAuthConfig.ca_secret`:
"serialized for controller->gateway communication, but should be skipped when
deserializing from YAML files"). A blanket `#[serde(skip)]` would break that sync. The
correct boundary to strip at is the **admin API response**, not the struct definition —
which is exactly where Option B applies the projection.

## Relationship to the broader admin-API hardening

This is the narrow, targeted fix for the **private-key disclosure** consequence only.
The general "gateway admin API binds `0.0.0.0:12001` with no auth and unconditionally
mounts `/configclient/*`" problem (companion finding `types-misc-01`) is tracked
separately in `tasks/todo/gateway-admin-api-harden.md` (move sensitive endpoints behind
the `is_integration_testing_mode()` gate + default the bind to `127.0.0.1`). The secret
scrub here is defense-in-depth that holds even if the route is reachable: the private key
never reaches the response body regardless of the auth/bind posture.

## Tests guarding this

- `src/core/gateway/api/mod.rs::edgion_tls_scrub_tests::admin_serialization_strips_private_keys`
  builds an `EdgionTls` with a populated leaf `secret` (`tls.key`) and an mTLS `ca_secret`,
  runs both `to_json_vec_edgion_tls` and `find_edgion_tls`, and asserts the serialized JSON
  contains neither private-key marker nor the `tls.key` / `secret` / `caSecret` fields,
  while non-secret fields (`hosts`) survive.

## Re-evaluation triggers

- A new admin endpoint that returns `EdgionTls` (or any resource embedding a resolved
  `Secret`) without routing through a scrubbing projection — that would reopen the leak for
  the new path. The structural guarantee here covers only the `/configclient` handlers.
