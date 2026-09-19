# B0034 — LinkSys Data Sender

## Scope

- Baseline: `6ceb4cf904cbdc7c6b9186520222fe6a82549061`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/link_sys/runtime/data_sender.rs`
- Approved support files: `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs`, `edgion-gateway/src/observe/access_log/logger.rs`, `edgion-gateway/src/observe/logs/logger_factory.rs`, `edgion-gateway/src/link_sys/readme`, and `skills/04-review/observability/regression-guards/access-log-all-senders-unhealthy-metric.md`.
- Forbidden files: every other source or documentation file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the approved code/docs above plus B0033/B0034 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0034 analysis turn.
- Decision: `change`, simple. `create_async_logger` received Access/TCP/TLS/UDP type information, but `AccessLogger` discarded it. Selected remote sender errors and all-unhealthy loss always incremented `edgion_access_log_dropped_total`, misattributing non-HTTP loss and leaving the documented protocol counters unchanged.
- Ownership decision: make existing `LogType` the single canonical drop-counter dispatcher, store it in `AccessLogger`, and propagate it from the factory.
- Preserved semantics: first-healthy-only selection, no error fallthrough, zero-sender silent no-op, one remote admission-loss count, LocalFile internal queue/write counting without dispatcher duplication, metadata borrowing, generation publication, and shutdown.
- Observability boundary: reuse the four existing bounded label-free counters; add no metric, label, request trace, or payload/error reflection.
- Freshness revalidation: valid at `6ceb4cf90`; B0033 changed only a Webhook unit test and campaign records.

## Implementation

- Added crate-visible `LogType::record_drop`, reused it from LocalFile, stored `LogType` in `AccessLogger`, and used a typed constructor from the async logger factory. Existing `new` and `Default` remain Access-compatible.
- Both selected-sender errors and all-unhealthy loss now increment the configured type's counter. Zero senders remain silent.
- Added subprocess-isolated regressions for each protocol, remote failure, Access defaults, zero senders, and factory propagation.
- Corrected stale review and LinkSys operational guidance to name the typed async drop-counter family while excluding the separate synchronous SSL path.

## Independent review

- Reviewer: `b0001_review`, independent from the B0034 analyst and implementer.
- Verdict: approved after confirming exact-once accounting, LocalFile non-duplication, constructors and factory propagation, subprocess metric isolation, performance/layout impact, inventory/no-tracing compliance, and the corrected operational documentation.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | AccessLogger 8, logger factory 5, and LocalFile sender 17 tests passed; new isolated metric cases passed five repeated reviewer runs. |
| Independent review checks | passed | Gateway check/Clippy, formatting, metrics inventory, no-tracing, agent docs, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
