---
name: deferred-credential-connection-nomination
description: Use when reviewing deferred upstream credential materialization, Connection nominations, or upstream_request_filter local-failure ownership.
knowledge_type: regression-guard
---

# Deferred credentials must reject Connection nomination before materialization

## Required behavior

`pg_upstream_request_filter` is the final owner of deferred credential header
materialization. It must validate every `Connection` field from both header
views before inserting a credential:

- `session.req_header()` is the canonical post-plugin downstream request. A
  late plugin can add a nomination here after `CredentialInjector` has staged
  a credential.
- `upstream_request` is Pingora's attempt-local clone. It can differ across
  retries and may contain a later attempt-local mutation.

Pingora sanitizes the attempt-local clone before invoking this hook. Checking
only that clone is insufficient because sanitization can remove the original
`Connection` field and its nominated credential header. Materializing the
credential afterward would reintroduce a sensitive field that Pingora had
already removed as hop-by-hop.

Reject before materialization when either view contains a case-insensitive
nomination of a deferred credential target or any `Connection` value is not
valid UTF-8. Inspect all field lines and all comma-separated tokens. Unrelated
options, including the ordinary WebSocket `Connection: Upgrade` handshake,
remain valid when they do not nominate a credential target.

## Failure ownership

Nomination and non-UTF-8 rejection records the request diagnostic
`CredentialConnectionConflict`. Expiry continues to record only
`CredentialExpired`. The hook returns the same static-detail,
non-retryable `HTTPStatus(500)` used for local framing, credential application,
and AI dispatch-state preparation failures.

This is local pre-dispatch control flow. It must not be attributed to the
selected backend, consume an AI dispatch, or enter upstream retry/outlier
accounting. Neither the error detail nor any log, metric, diagnostic, or access
log field may contain a credential header name or value.

## Regression triggers

- Credential validation reads only `upstream_request` or only the session
  header.
- Validation moves after deferred credentials are applied.
- Only the first `Connection` field or token is checked.
- Local preparation failures become retryable or use `InternalError`.
- Credential target names or values are added to error detail or observability.
- `CredentialConnectionConflict` is emitted for expiry or unrelated local
  preparation failures.
