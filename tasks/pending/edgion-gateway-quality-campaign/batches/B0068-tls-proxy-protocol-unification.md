# B0068 — TLS Outbound PROXY Protocol Unification

## Scope

- Baseline: `0b5890414bf0e9a9d2459fb318e509058632f624`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/routes/tls/passthrough_proxy.rs`.
- Approved code files: `edgion-gateway/src/routes/tls/proxy.rs`, the anchor file, and focused unit coverage in `proxy.rs` or `routes/tls/tests.rs`.
- Approved knowledge files: `skills/04-review/tls/regression-guards/pp2-dst-bare-ip.md` and `skills/04-review/tls/intentional-design/tls-setup-writes-remain-transport-bounded.md`.
- Existing `edgion-tests/**` and protected user paths remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- Terminate and passthrough duplicate the complete outbound PPv2 build, write, flush, accounting, and log policy.
- The duplication has concrete drift history: passthrough once emitted the wrong unspecified IPv4 destination for bracketed IPv6 while terminate handled it correctly.
- Extract one canonical helper from the terminate owner and reuse it from both modes without changing bytes, ordering, failures, or observability.

## Required invariants

- Preserve source identity, bracket-safe destination parsing and unspecified fallback, SNI authority TLV, and exact PPv2 bytes.
- Invoke only after successful upstream connect; passthrough sends PPv2 before replaying the original ClientHello.
- Preserve write/flush error propagation and prevent relay after setup failure.
- Set `proxy_protocol_sent` and append `PP2 sent` only after successful write and flush.
- Do not change routing, owner generations, Stage 2 identity, retry/outlier policy, timeouts, cleanup, redaction, access-log ownership, allocations, locks, metrics, or tracing.

## Verification plan

- Add focused helper coverage for IPv6 destination/authority bytes and write-or-flush failure accounting.
- Rerun all TLS module tests and the socket ClientHello matrix.
- Run full campaign gates and frozen-path verification.

## Deferred findings

- None newly identified; D0012 remains outside this batch.

## Implementation

- Extracted the terminate-owned PPv2 setup into one module-level generic helper and routed both TLS modes through it.
- Passthrough retains post-connect, pre-ClientHello-replay ordering; either write or flush failure still returns before replay and relay.
- Removed the duplicate passthrough builder/imports while preserving exact source/destination fallback, authority TLV, bytes, errors, success marker, and access-log note.
- Added direct tests for IPv6/authority bytes, disabled and invalid-source no-ops, invalid-destination fallback, partial write failure, and flush failure.

## Independent reviews

- The independent plan review approved the ownership boundary, exact two-code/two-knowledge-file scope, failure matrix, and preserved transport ordering.
- The final reviewer returned LGTM with no Must Fix, Should Fix, or deferred findings and independently confirmed wire bytes, failure accounting, ordering, observability, knowledge accuracy, and scope.

## Verification

- Focused helper tests passed 7/7, the TLS module passed 77/77, and the socket ClientHello matrix passed 1/1.
- Formatting, diff, workspace check/clippy, agent-doc, SSA-force, metrics-inventory, Pingora-source-policy, Gateway API transform contract, no-tracing, cargo-deny, English-only, and frozen-path gates passed.
- Clippy retained only the known unrelated Kafka and frozen integration-test warnings. No full test suite was requested or run.

## Handoff

- Implementation and knowledge committed as `83595505`.
- Next campaign anchor: `edgion-gateway/src/routes/tls/routes_mgr.rs`.
