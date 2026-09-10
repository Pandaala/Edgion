# Isolate ExtProc Live Stream Tests from the Unit Tier

## Meta

- Status: pending
- Priority: P2
- Category: test isolation

## Evidence and scope

`edgion-gateway/src/plugins/http/ext_proc/client.rs::tests::live_stream` starts live
gRPC servers. These scenarios provide transport and lifecycle coverage, but running
real servers inside the unit tier conflicts with the repository's nonnetwork unit-test
rule. This is separate from deterministic duplex/channel-based tests in the same module.

Inventory each live scenario and identify meaningful replacements or relocation to an
authorized integration scope. Preserve timeout, half-close, cancellation, TLS and other
existing assertions; do not delete coverage merely to make a broad unit filter pass.
Prefer pure seams for client state transitions and keep actual transport behavior in
the integration tier with its shared topology discipline.

## Acceptance

- Read current root rules and original unit/integration guidance; independently review the coverage mapping.
- Retain equivalent assertions for every moved or replaced scenario.
- Demonstrate the resulting unit selection starts no server and performs no network exchange.
- Run the affected deterministic tests and relevant project checks.
- Do not change frozen integration files as part of the ongoing quality campaign; schedule any required integration edits under separate explicit scope.
