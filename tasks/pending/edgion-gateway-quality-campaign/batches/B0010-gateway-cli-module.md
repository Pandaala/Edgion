# B0010 — Gateway CLI Module

## Scope

- Baseline: `6f93c8c92882031b2d5d288811580071f28f45d7`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/cli/mod.rs`
- Allowed code files: `edgion-gateway/src/cli/mod.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above and campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in B0010 analysis and freshness turns.
- Confirmed defects: the intended work-directory checkpoint is emitted before tracing is installed and is discarded; a failed `require_fips` check calls `process::exit`, skipping the retained logging worker guard and risking loss of the required fatal diagnostic.
- Decision: `change` (simple). Move the existing work-directory INFO event immediately after successful logging initialization with `component = "startup"`, and propagate the unchanged FIPS error through the existing outer fatal-error path so the guard remains alive and flushes on return.
- Preserved semantics: work-directory/config precedence, FIPS requirement/policy/message/metric order, BoringSSL-only support, final non-zero status, startup observability ownership, schema and protocol behavior.
- Freshness revalidation: valid at `6f93c8c92`; B0009 did not change the CLI module, startup helper, binary entrypoint, or FIPS runtime.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0010 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: moved the existing work-directory checkpoint immediately
  after successful logging initialization with `component = "startup"`; FIPS
  requirement failures now propagate unchanged through the existing outer
  fatal-error path instead of calling `process::exit` locally.

## Independent review

- Reviewer: `b0001_review`, independent from the B0010 analyst and implementer.
- Verdict: approved with no findings. The reviewer confirmed guard lifetime,
  single fatal-record ownership, unchanged diagnostic/precedence/metrics/policy,
  and the same final status 1, including an actual non-FIPS startup smoke.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Baseline FIPS helper tests | passed | 3 requirement tests passed during freshness validation. |
| Baseline startup cleanup test | passed | OTLP startup-error cleanup child test passed during freshness validation. |
| Frozen integration paths | passed | Existing integration files remained unchanged during analysis. |
| Focused implementation checks | passed | 3 FIPS helper tests, OTLP startup cleanup, Gateway all-target check, formatting, no-tracing, English-only, and diff checks passed. |
| Independent review checks | passed | Focused tests, Gateway binary build, actual require-FIPS startup/log-flush/status smoke, formatting, diff, no-tracing, and metrics-inventory checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `637278a6b9d3a5b92c5f71aefda22286870fddbc`.
