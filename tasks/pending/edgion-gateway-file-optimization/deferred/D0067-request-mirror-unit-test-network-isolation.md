# D0067 — Isolate RequestMirror Unit Tests from the Network

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0102, RequestMirror analysis
- Category: test-isolation
- Canonical task: [`tasks/todo/isolate-request-mirror-unit-tests-from-network.md`](../../../todo/isolate-request-mirror-unit-tests-from-network.md)

## Evidence
Four RequestMirror arming unit tests start the detached reqwest writer against TEST-NET destinations. Their synchronous assertions remain useful, but the spawned work violates the repository rule that unit tests must not make real network attempts and can create environment-dependent background behavior.

## Why deferred
The correct fix needs a narrow decision/writer seam that proves arming without dialing while retaining body-tee, cancellation, semaphore, and access-log ownership. Deleting the tests or weakening their assertions would lose important coverage, and this is separate from the shared-client lifecycle change in B0102.

## Integration-test impact
None. This is unit-test isolation; existing frozen RequestMirror integration coverage remains unchanged.
