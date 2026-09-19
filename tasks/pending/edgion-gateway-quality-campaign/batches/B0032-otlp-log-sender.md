# B0032 — OTLP Log Sender

## Scope

- Baseline: `63a48dc1eb0fe06f91a45dc640052317acf628c2`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs`
- Allowed knowledge correction: `skills/04-review/link-sys/intentional-design/process-exit-log-delivery-best-effort.md`
- Allowed source files: none; analysis concluded examined-no-change.
- Forbidden files: every source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the knowledge file plus B0031/B0032 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0032 analysis turn.
- Source decision: `examined-no-change`. `OtlpLogSender` is a key-only adapter that resolves the current immutable `OtlpLinkClient` generation. Shared generations own canonical TLS/egress, probes, bounded SDK batching, retirement, timeout, and cancellation.
- Record evidence: body moves once; trace ID remains typed without inventing a span; canonical sensitive-path transformation, timestamps, severity, method, and status remain bounded and correct. Missing/unhealthy/removal races use the existing drop contract without reflecting sensitive details.
- Rejected churn: adding a sender queue, export acknowledgement, client/probe, cached generation handle, or custom processor would duplicate canonical ownership and violate the accepted best-effort OTLP boundary.
- Independent knowledge finding: the active process-exit page still described a provider-owning sender and no exit hook. Current code uses key-only senders, persistent generation-owned pipelines, and explicit bounded OTLP shutdown after UDP drain plus the startup-error guard.
- Preserved conclusion: local SDK flush/retirement is an attempted bounded drain, not collector acknowledgement or a lossless tail-delivery guarantee.
- Freshness revalidation: valid at `63a48dc1e`; B0031 changed allocator reporting only.

## Implementation

- Source implementation: none.
- Corrected the OTLP lifecycle knowledge page to reflect current ownership and normal/startup-error shutdown order while preserving historical evidence and the accepted best-effort limitation.

## Independent review

- Reviewer: `b0001_review`, independent from the B0032 analyst and knowledge editor.
- Verdict: source examined-no-change approved; knowledge correction approved after checking sender/generation ownership, runner deletion and join, UDP-before-OTLP ordering, startup guard lifetime, SDK retirement, remote acknowledgement limitations, and exact scope.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | OTLP sender 5, provider 13, access logger 7, store-filtered 71, and real OTLP transport 9 tests passed. |
| Independent review checks | passed | Gateway check, formatting, agent docs, no-tracing, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `b7b2b4d593bad4538a21478e4b23ec3d10a37393`.
