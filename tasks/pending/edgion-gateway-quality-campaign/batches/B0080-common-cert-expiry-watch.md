# B0080 — Common certificate expiry watcher

## Scope

- Baseline: `f0ec920dcaf97f3c21c3f5c5ae5e7be5a6d06b09`.
- Pipeline state: terminal.
- Active anchor: `edgion-common`.
- Target: `edgion-common/src/conf_sync/cert_expiry_watch.rs`.
- Initial analysis proposed no implementation scope; the continuation below records the exact authorized test-only paths. Existing integration files and protected user paths remain read-only.

## Context and value gate

Analyst `b0080_analysis`: change, simple, test-only. Full watcher/tests and live identity/reload consumers examined. Production severity/failure transitions and cold lifecycle observability are sound. Predictable PID directory and three absolute missing paths violate test isolation; unreadable-source test lacks assertions.

Proposed exact edit paths: `edgion-common/tests/unit/conf_sync/cert_expiry_watch_test.rs`, `edgion-common/Cargo.toml`, `Cargo.lock` (Common dependency entry only). Add workspace tempfile dev dependency, use owned TempDir paths, preserve test identities/reread timestamps, and assert failure count plus retained prior severity. No global recorder/environment/network or production edits. Targeted test filter: conf_sync::cert_expiry_watch::tests.

D0018 independently records file-vs-installed identity expiry divergence; no cross-crate runtime fix is approved. Knowledge: common layout, sync overview, mtime/length watcher design, observability review rule and metric exception, unit testing. Apply all logging/redaction/restraint/unification gates. Independent review and full gates required on promotion.


## Continuation implementation

Replaced the predictable directory and missing-file paths with owned TempDir
fixtures; retained all 20 test identities. The unreadable-source test now asserts
failure count and preservation of the prior Critical severity. Added only the
workspace tempfile dev dependency and its Common lockfile edge.
Independent `watch_review` reviewed the full three-file diff and returned LGTM.
D0018 remains unresolved; this test-only batch does not fix installed-identity
observation. Targeted runtime validation and shared commit gates passed as recorded below.

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
