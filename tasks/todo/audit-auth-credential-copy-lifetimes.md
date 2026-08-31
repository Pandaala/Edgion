# Audit Auth Credential-copy Lifetimes

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Inventory and reduce avoidable credential copies and retention across resource parsing, Controller Secret resolution, conf-sync, Gateway runtime configs, request decoding, and authentication plugins. Define bounded copy/lifetime goals without claiming end-to-end zeroization that the allocator and serialization stack cannot guarantee.

## Requirements

- Map every owned and borrowed copy of resolved passwords, password hashes, tokens, decoded Authorization values, client identities, and adjacent credentials across the full Controller-to-Gateway path.
- Classify each copy as required by serialization/transport, required for generation stability, or avoidable.
- Define explicit maximum lifetimes and ownership boundaries for each stage, including error and cancellation paths.
- Reuse sensitive wrappers and redacted Debug/Display where they provide real protection; do not add wrappers that leave upstream clones untouched while implying erasure guarantees.
- Evaluate practical zeroization only for uniquely owned buffers whose lifetime and allocator behavior make it meaningful.
- Preserve conf-sync correctness, hot-reload generation pinning, auth timing, and failure behavior.

## Acceptance criteria

- The audit records the complete credential-copy graph and a justified disposition for every copy.
- Implemented reductions remove avoidable copies without weakening generation or retry correctness.
- No credential value appears in Debug, logs, metrics, errors, task records, or admin output.
- Documentation states the precise bounded-lifetime guarantees and explicit limits of zeroization.
- Focused cross-crate tests, workspace check, Clippy, and repository guards pass.
