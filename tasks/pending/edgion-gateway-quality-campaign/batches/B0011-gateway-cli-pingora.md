# B0011 — Gateway CLI Pingora Integration

## Scope

- Baseline: `637278a6b9d3a5b92c5f71aefda22286870fddbc`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/cli/pingora.rs`
- Allowed code files: `edgion-gateway/src/cli/pingora.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above, B0010/B0011 campaign records, D0006 campaign/task records, and campaign ledger/main/changelog/deferred-index records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0011 analysis turn.
- Confirmed defect: the documented `server.threads: 0` automatic mode is copied directly into Pingora's server configuration, but both the Tokio work-stealing runtime and Pingora's no-steal runtime reject zero worker threads during construction.
- Decision: `change` (simple). Normalize zero exactly like an omitted value through the existing platform-parallelism fallback, preserve positive values, and add local mapping regressions.
- Preserved semantics: every other Pingora server option, listener and shutdown lifecycle, logging ownership, BoringSSL-only support, schema, CLI spelling, and configuration defaults.
- Freshness revalidation: valid at `637278a6b`; B0010 changed only `cli/mod.rs` and did not overlap the Pingora integration.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0011 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: filtered an explicit zero before the existing CPU-count fallback so omission and zero resolve identically to at least one worker; explicit positive counts remain unchanged.
- Tests: added platform-independent mapping coverage while retaining the existing Unix shutdown tests unchanged.

## Independent review

- Reviewer: `b0001_review`, independent from the B0011 analyst and implementer.
- Verdict: approved with no findings. The reviewer confirmed both Pingora runtime modes are protected before work-stealing selection and all other server/lifecycle settings remain unchanged.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | 4 Pingora CLI tests, Gateway all-target check, formatting, no-tracing, English-only, and diff checks passed. |
| Independent review checks | passed | New mapping tests, existing Unix shutdown tests, Gateway library check, formatting, diff, no-tracing, and Pingora source policy passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Deferred issue: D0006 was resolved by defining local YAML as the process-configuration owner; see `skills/04-review/resources/intentional-design/gateway-process-config-is-yaml-only.md`.
- Commit: `b3274df3ab8529485b7df151aad9f93791895e95`.
