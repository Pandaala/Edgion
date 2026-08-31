# D0011 — Define a DataSender Graceful Shutdown Contract

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0049, `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`
- Category: reliability

## Evidence
Process-lifetime senders rely on channel closure and object or thread teardown to flush buffered work, but the shared sender abstraction has no shutdown, join, drain acknowledgement, or deadline contract. Local-file writer termination therefore cannot prove that queued and buffered records were persisted before process exit.

## Why deferred
A correct lifecycle contract crosses every DataSender implementation, logger ownership, binary shutdown ordering, bounded drain deadlines, and failure reporting. Adding a local join mechanism would create a fifth, provider-specific lifecycle shape.

## Impact and open questions
- Process exit may not wait for queued or buffered records to complete.
- Reload/removal and process shutdown may require different drain semantics.
- The contract must stay bounded and preserve non-blocking request-path admission.

## Candidate next step
Design one shared sender shutdown protocol covering cancellation, bounded drain, acknowledgement, errors, and binary shutdown ordering, then apply it consistently across providers with unit-level lifecycle tests.
