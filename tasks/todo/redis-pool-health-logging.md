# Define Redis Pool Health and Safe Connection Logging

## Meta
- Status: pending
- Severity: medium
- Category: reliability

## Evidence
All pool clients share one health boolean, so the last reconnect or error event determines provider health instead of aggregate usability. Connection callbacks also emit Redis endpoint identities at INFO/WARN and can scale with pool size.

## Why separate
Health gating and operational logging must be redesigned together around per-client state, aggregate semantics, bounded transitions, and metadata-only messages.

## Candidate next step
Choose `any usable` or `all connected` health semantics, track pool members deterministically, debounce state transitions, remove endpoint identities, and add multi-client recovery tests.
