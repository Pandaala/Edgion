# D0016 — Define Redis Pool Health and Safe Connection Logging

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0052, `edgion-gateway/src/link_sys/providers/redis/client.rs`
- Category: reliability

## Evidence
All pool clients share one health boolean, so the last reconnect or error event determines provider health instead of aggregate usability. Connection callbacks also emit Redis endpoint identities at INFO/WARN and can scale with pool size.

## Why deferred
Health gating and operational logging must be redesigned together around per-client state, aggregate semantics, bounded transitions, and metadata-only messages.

## Candidate next step
Choose `any usable` or `all connected` health semantics, track pool members deterministically, debounce state transitions, remove endpoint identities, and add multi-client recovery tests.
