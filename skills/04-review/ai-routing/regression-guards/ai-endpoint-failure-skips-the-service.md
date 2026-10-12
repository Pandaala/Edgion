---
knowledge_type: regression-guards
---

# AI endpoint failure skips the service

Predispatch failures are classified by the configuration they depend on.
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

## Resolution references

- `gw-08` — 2026-08-09.
