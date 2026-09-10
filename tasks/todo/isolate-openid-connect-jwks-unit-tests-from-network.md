# Isolate OpenID Connect JWKS Unit Tests from Network

## Meta

- Status: pending
- Priority: P2
- Category: testing-refactor
- Severity: low
- Discovered in: B0298 at `84af258d`

## Evidence

The default `edgion-gateway` unit-test suite contains 24 OpenID Connect JWKS tests that perform real loopback networking:

- `edgion-gateway/src/plugins/http/openid_connect/tests.rs` has two default-run JWKS Dye tests that call `wiremock::MockServer::start()`.
- `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs` has 22 default-run network tests using a `TcpListener` fixture, Wiremock servers, or a deliberate connection to `127.0.0.1:1`.

These violate the repository unit-test rule that tests must not access a network or start real servers. Link-local egress cases that are rejected before dialing are not part of this issue.

## Required preserved coverage

Do not delete the tests or mark them ignored. The replacement must retain the current assertions for discovery and JWKS snapshots, TTL and minimum-refresh behavior, stale-cache fallback, no-cache failure, refresh singleflight, discovery failure/backoff, document validation, HTTP status and timeout handling, key rotation, and Dye outcomes. Re-check any review knowledge that cites the existing discovery-backoff test evidence after migration.

## Constraints

- Prefer pure state transitions, parsing helpers, outcome mapping, or a cold-path test seam over a second production transport mechanism.
- Do not add allocations, trait dispatch, locks, or branching to the request hot path for mock convenience.
- Preserve canonical outbound HTTP, TLS, egress admission, response caps, failure policy, redaction, PluginLog/Dye and access-log ownership.
- Do not modify existing `edgion-tests/**` files. Add a future standalone integration only if the refactor proves a transport contract is otherwise uncovered and it can be registered without editing frozen files.
- Do not fold in D0058 dependency-error redaction or unrelated OIDC cleanup.

## Candidate next step

Inventory the 24 tests by contract, design the smallest production-neutral seam that makes each contract deterministic without sockets, obtain plan review, migrate the tests in coherent groups, and independently verify coverage parity plus the normal workspace guards.
