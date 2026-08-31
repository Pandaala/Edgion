# D0036 — Audit Auth Credential-copy Lifetimes

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0067, BasicAuth audit
- Category: sensitive-data
- Canonical task: [`tasks/todo/audit-auth-credential-copy-lifetimes.md`](../../../todo/audit-auth-credential-copy-lifetimes.md)

## Evidence
Credential material crosses resource deserialization, Controller Secret resolution, conf-sync serialization, runtime config construction, header decoding, and multiple auth plugins as ordinary owned strings. B0067 removes unnecessary BasicAuth-local copies but does not bound or erase every upstream, transport, serde, protobuf, or allocator copy.

## Why deferred
A meaningful improvement requires a cross-crate inventory and explicit lifetime/copy budget. It must not claim end-to-end zeroization that Rust allocators, transport buffers, serde, or protobuf cannot guarantee.

## Integration-test impact
None required. Copy/lifetime contracts need unit, property, and instrumentation-based verification; existing behavior integration tests should remain unchanged.
