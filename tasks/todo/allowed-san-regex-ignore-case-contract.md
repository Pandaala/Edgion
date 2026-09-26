# Resolve AllowedSan regex ignoreCase semantics

## Meta

- Status: pending
- Type: investigation
- Priority: P2
- Severity: medium
- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`

## Evidence

The EdgionTls CRD exposes `ignoreCase` without a match-type restriction. `edgion-resources/src/resources/allowed_san.rs::AllowedSan::compile_regex` calls the bounded regex compiler without consuming `ignore_case`. `edgion-common/src/allowed_san.rs` executes the compiled regex directly. A URI RegularExpression matching `^spiffe://cluster` remains case-sensitive with `ignoreCase: true` unless the pattern embeds `(?i)`.

The confirmed behavior is an accepted flag being ignored. This is not a demonstrated authorization bypass, and the intended regex flag contract remains unresolved. Operators can unexpectedly reject client certificates if they expect the exposed flag to apply.

## Scope and next steps

1. Establish whether ignoreCase applies to RegularExpression and how inline regex flags interact with it.
2. Apply the option during canonical bounded cold compilation, or explicitly reject and document the unsupported combination.
3. Cover schema validation, Gateway reconstruction after serde, and Common matching with both flag values.
4. Preserve regex size bounds, fail-closed invalid/uncompiled entries, typed SAN identity semantics, and zero handshake compilation/tracing.

## Why separate work

Resolving this requires coordinated schema/compiler/documentation/test semantics beyond a behavior-preserving Common matcher cleanup. Search original TLS review knowledge and verify current implementation before choosing the fix.
