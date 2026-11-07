---
knowledge_type: regression-guards
---

# AI endpoint failure skips the service

Predispatch and dispatched failures are classified by the configuration they depend on.
Credential admission failures skip only that credential. DNS resolution,
resolved-address safety validation, and TCP/TLS connection establishment
depend solely on the EdgionBackend endpoint, so any such failure must skip the
whole service for the current scheduler round. Connection establishment is
still predispatch: credentials are not dispatched until the upstream request
hook runs after the connection succeeds.

AI HTTP integration is owned by `edgion-gateway/src/ai/http/attempt.rs`: the
common peer/connect hooks delegate to named phase operations and retain their
transport accounting/deadline ordering. No ordinary retry policy moves into AI.

Peer construction and connection-failure handling use a typed predispatch
failure whose variant owns this scope decision. Its `finish()` method releases
the active reservation and advances the scheduler to service selection. The
next `upstream_peer` scan selects the successor; Pingora retry is enabled only
when that endpoint-scoped advancement succeeds. Future credential-dependent
fallible work must use a separate variant instead of being folded into the
endpoint branch.

The active credential reservation is still released before advancing. A
service-scoped predispatch skip consumes no dispatch slot, failure-ledger slot,
or retry-budget sample, and the route picker may immediately try another AI
service.

When Pingora's request relay forbids another attempt, connection-failure
handling releases the active reservation without advancing the scheduler or
marking the error retryable. The endpoint-scoped `finish()` operation owns the
release exactly once when relay state does permit advancement; callers must not
settle the reservation separately before invoking it.

Tests for this boundary need at least two credentials on the failed service and
a second service. They must prove the failed service cannot be selected again,
the failed service's second credential is not scanned, the second service
remains selectable, dispatch and failure-ledger counts stay zero, and retry is
marked only after endpoint-scoped advancement succeeds.

## Dispatched failures follow the same scope

The scope rule also applies after the dispatch boundary. A permitted retry of a
dispatched failure carries a `FailureScope` into the scheduler
(`edgion-gateway/src/ai/runtime/attempt.rs`):

- `Credential` — provider status 401, 403 or 429. The retry stays on the same
  backend and prefers an untried credential (`Phase::PickCredential`).
- `Endpoint` — every other retryable status (5xx, 529, ...), a backend request
  timeout, and an explicitly replayed ambiguous transport failure. The backend
  is added to the round's excluded set and the retry goes to the route picker
  (`Phase::PickBackend`).

`FailureScope::from_http_status` is the single status classification;
`handle_ai_response` applies it, and `handle_ai_transport_failure` and
`retry_ambiguous_dispatch` are always endpoint-scoped. Unlike the predispatch
skip, a dispatched endpoint failure still consumes its dispatch slot, failure-
ledger slot and retry-budget sample, and it marks the round as replayable, so a
route with a single backend still retries that backend in the next round.

Root cause of `gw-ai3-08`: every dispatched retry returned to
`PickCredential` on the failed backend, so a provider outage spent the whole
retry budget on sibling credentials sharing the dead endpoint and the
configured fallback backend was never tried.

Regression evidence: scheduler tests
`endpoint_failure_fails_over_without_scanning_sibling_credentials`,
`endpoint_failure_on_single_backend_retries_in_a_new_round` and
`failure_scope_classifies_credential_statuses`; dispatch tests
`response_retry_scope_follows_the_failure_domain`,
`endpoint_scoped_response_retry_selects_the_next_backend`,
`transport_timeout_retry_fails_over_the_backend` and
`ambiguous_failure_replays_only_when_explicitly_requested`. The integration
case `EdgionBackend/CredentialExhaustion`
`endpoint_failover_then_credential_exhaustion` encodes the end-to-end order
(primary 503 → secondary → next-round primary on its untried credential) in the
access-log attempt ledger; it was updated with this fix but not run as part of
it.

Known limit: 429 is always credential-scoped. A provider-wide or deployment-
level 429 (for example a shared deployment quota) still rotates sibling
credentials first. Re-evaluate if such a provider becomes a supported target,
if a provider reports a credential-scoped condition with a different status,
or if a per-route scope override is requested.

## Resolution references

- `gw-08` — 2026-08-09.
- `gw-ai3-08` — 2026-11-07, dispatched retries classify the failure scope.
