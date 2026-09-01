# Unify Response-Header Mutation Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-02 |
| Status | pending |
| Type | architecture-correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0104 |

## AI guide summary

Define one canonical response-header mutation policy across plugins, Gateway API filters, DSL, and the final session boundary.

## Requirements

- Inventory every response-header mutation producer and its validation, ordering, error, and protocol behavior.
- Define which framing and hop-by-hop fields may be removed, inserted, replaced, or renamed for HTTP/1.1, HTTP/2, and gRPC.
- Keep the main upstream response streaming and prevent declared framing from diverging from emitted body chunks.
- Reuse one shared validator/runtime policy instead of local deny-lists.
- Preserve binary-owned logging, canonical access-log diagnostics, fixed non-sensitive errors, and observability restraint.
- Update Controller/Gateway validation parity and English/Chinese documentation atomically.
- Keep existing integration tests frozen; create a separate integration issue only for a proven wire-level gap.

## Acceptance criteria

- One decision matrix covers every producer, operation, restricted field, and protocol.
- All admission and runtime mutation paths enforce the approved policy consistently.
- Deterministic tests cover case-insensitive names, rename source/destination, removal, framing, hop-by-hop fields, and operation order.
- Workspace format/check/Clippy and repository guards pass.
