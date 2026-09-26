# B0080 — Common certificate expiry watcher

## Scope

- Baseline: `f0ec920dcaf97f3c21c3f5c5ae5e7be5a6d06b09`.
- Pipeline state: analysis-ready.
- Active anchor: `edgion-common`.
- Target: `edgion-common/src/conf_sync/cert_expiry_watch.rs`.
- No implementation scope approved; existing integration files and protected user paths remain read-only.

## Context and value gate

Analyst `b0080_analysis`: change, simple, test-only. Full watcher/tests and live identity/reload consumers examined. Production severity/failure transitions and cold lifecycle observability are sound. Predictable PID directory and three absolute missing paths violate test isolation; unreadable-source test lacks assertions.

Proposed exact edit paths: `edgion-common/tests/unit/conf_sync/cert_expiry_watch_test.rs`, `edgion-common/Cargo.toml`, `Cargo.lock` (Common dependency entry only). Add workspace tempfile dev dependency, use owned TempDir paths, preserve test identities/reread timestamps, and assert failure count plus retained prior severity. No global recorder/environment/network or production edits. Targeted test filter: conf_sync::cert_expiry_watch::tests.

D0018 independently records file-vs-installed identity expiry divergence; no cross-crate runtime fix is approved. Knowledge: common layout, sync overview, mtime/length watcher design, observability review rule and metric exception, unit testing. Apply all logging/redaction/restraint/unification gates. Independent review and full gates required on promotion.

