# B0079 — Common certificate utilities

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: analysis-ready.
- Active anchor: `edgion-common`.
- Target: `edgion-common/src/cert_util.rs`.
- Read-only candidate; no implementation paths approved.
- Existing integration files and pre-existing user task paths remain protected.

## Context and value gate

Analyst `b0079_analysis`: examined-no-change. Fully read cert_util and three tests, expiration watcher, Controller/Gateway identity and federation consumers. First-PEM validity parsing supplies hourly expiry observation; identity installation and handshake own authentication. Actual time values do not establish an overflow path. Extra chain parsing or cross-crate parser unification lacks sufficient value. No code scope or confirmed deferred issue.

Knowledge: common layout, controller-gateway-link overview, time conventions, TLS/conf-sync indexes, cert-watch mtime/length design, unit-testing. Preserve binary-owned observability, redaction, no request-path logging/allocation changes, canonical mechanisms and test immutability. Independent review required on serial promotion.

