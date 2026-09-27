# B0079 — Common certificate utilities

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: terminal.
- Active anchor: `edgion-common`.
- Target: `edgion-common/src/cert_util.rs`.
- Read-only candidate; no implementation paths approved.
- Existing integration files and pre-existing user task paths remain protected.

## Context and value gate

Analyst `b0079_analysis`: examined-no-change. Fully read cert_util and three tests, expiration watcher, Controller/Gateway identity and federation consumers. First-PEM validity parsing supplies hourly expiry observation; identity installation and handshake own authentication. Actual time values do not establish an overflow path. Extra chain parsing or cross-crate parser unification lacks sufficient value. No code scope or confirmed deferred issue.

Knowledge: common layout, controller-gateway-link overview, time conventions, TLS/conf-sync indexes, cert-watch mtime/length design, unit-testing. Preserve binary-owned observability, redaction, no request-path logging/allocation changes, canonical mechanisms and test immutability. Independent review required on serial promotion.


Promotion at `dfe2d11bb`: target/test and investigated runtime consumers/knowledge unchanged since analysis. Includes full tests/unit/cert_util_test.rs. Code allow-list empty; independent no-change review required.

## Continuation review

On 2026-09-27, the primary agent and independent reviewer `cert_review` fully
reviewed the source and its three unit tests against current consumers. No local
change is needed: this parser observes the first leaf validity window; identity
installation and handshake validation own authentication and chain correctness.
The `cert_` baseline ran 44 tests successfully, including all three utility tests.
Final shared batch gates passed as recorded below; local commit is pending.

## Verification on 2026-09-27

Common library suite: 314 passed, 4 pre-existing ignored. Controller CA suite:
5 passed, including the new overflow regression. Workspace all-target check and
Clippy passed; fmt, agent docs, SSA, metrics inventory, Pingora source policy,
Gateway API transform, unit-test layout and Gateway tracing guards passed.
All 3,772 frozen integration fingerprints match. No end-to-end/conformance run
is claimed; production change is bounded to pure CA date arithmetic.

Fresh test rebuild initially lacked protoc; installed protoc-wheel-0 29.3 only
under /tmp and supplied PROTOC explicitly. The transform guard used the existing
/tmp validation Python environment for PyYAML. No repository dependency or system
configuration was changed to repair the build environment.
