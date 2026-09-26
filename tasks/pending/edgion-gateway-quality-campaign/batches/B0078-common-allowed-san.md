# B0078 — common-allowed-san

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: analysis-ready.
- Active anchor crate: `edgion-common`.
- Target: `edgion-common/src/allowed_san.rs`.
- Read supporting callers and tests as needed; no implementation scope is approved yet.
- Forbidden: existing `edgion-tests/**`, unrelated modules, and protected user task paths listed in the main campaign record.

## Context and value gate

Analyst: `common_coverage`. Outcome: defer. Read complete matcher and 13 unit tests, schema compiler and tests, RFC 6125 matcher, Controller validation, Gateway compilation and BoringSSL consumers. Typed identity, wildcard/IP behavior, display prefix separation and precompiled fail-close checks remain sound. Lowercase allocation changes have no demonstrated net benefit.

Confirmed: exposed ignoreCase is unused for RegularExpression by schema compilation and matching. D0016 tracks intended-contract resolution across schema/compiler/docs/tests. No code edit authorized; independent review pending.

Knowledge: TLS resource architecture/features, downstream TLS/mTLS, typed-SAN display guard, leaf-only whitelist guard, RFC6125 wildcard-depth guard, generation recompilation design, unit testing. Preserve all red gates, no handshake regex compilation/tracing, canonical TLS access logging and frozen integration files.

