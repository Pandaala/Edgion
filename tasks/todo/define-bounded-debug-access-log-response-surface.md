# Define a Bounded Debug Access Log Response Surface

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | design-correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | D0068 canonical response-header mutation and error policy |

## AI guide summary

Define whether the debug access-log response surface remains supported and, if so, give it a dedicated non-terminal schema, protocol-aware hard bound, and canonical failure behavior.

## Requirements

- Decide whether the plugin remains generally configurable, becomes explicitly development/integration-only, uses a compact dedicated schema, or is removed.
- Define a hard serialized-byte cap that is valid for supported HTTP/1.1, HTTP/2, and gRPC response behavior.
- Never truncate JSON. Serialization, oversize, or header-set failure must omit the header and add only a bounded static diagnostic.
- Reuse or depend on D0068's unified response-header mutation/error policy; do not add a plugin-local competing policy.
- Specify the non-terminal snapshot contract, including top-level status timing and exclusion or incompleteness of `ext`, `waf`, body-derived facts, AI/stream summaries, current/later response-stage logs, and terminal timing.
- Preserve B0125 Wrap suppression and define Off/Replace treatment without weakening canonical local-log masking semantics.
- Preserve immutable publication, response streaming, binary-owned operational logging, and observability restraint.
- Do not modify existing integration-test files.

## Acceptance criteria

- The supported surface has one documented schema and one enforced serialized-byte bound across every supported protocol.
- Every failure path omits the header deterministically, emits valid bounded diagnostics, and cannot leak partial or malformed JSON.
- The design has no second response-header mutation/error mechanism and explicitly composes with D0068.
- Focused unit tests cover exact-boundary and one-byte-over cases, serialization/header-set failures, mode behavior, and terminal-only field exclusions.
- D0087 or a successor standalone scenario covers any new public wire behavior without rewriting existing integration tests.
