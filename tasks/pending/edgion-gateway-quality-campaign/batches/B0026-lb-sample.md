# B0026 — LB Response Sampling

## Scope

- Baseline: `d72e23a1a97474c6586f80dd06897c1964b6fbcd`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/lb/sample.rs`
- Allowed support file: `edgion-resources/src/resources/http_route_preparse.rs`
- Forbidden files: every other source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the allowed support file plus B0025/B0026 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0026 analysis turn.
- Decision: `change`, simple. The canonical sample classifier and ordinary retry path each search the pre-parsed `retry.codes` vector on response handling. HTTP retry preparse validated the 400–599 domain but retained arbitrary duplicates, so file/etcd configuration could amplify response-hot-path CPU work without changing behavior.
- Ownership decision: normalize at the shared cold preparse boundary rather than duplicate normalization or a new container in the Gateway request path.
- Preserved semantics: first-seen order, invalid-code warnings, `None` versus explicit empty codes, retry decisions, Peak-EWMA useful-work classification, B0020 monotonic anchors, B0019 counter ownership, and separate unsupported GRPCRoute retry-code behavior.
- Freshness revalidation: valid at `d72e23a1a`; B0025 changed campaign records only.

## Implementation

- Valid 400–599 HTTP retry codes now pass through a fixed 200-slot seen set during preparse, bounding the runtime vector and its membership scans to the finite status-code domain.
- A duplicate-heavy full-domain regression proves first-seen ordering and the 200-entry maximum.
- No schema, CRD, transport, operator-facing field, logging, metrics, or Gateway sample-classifier change was introduced.

## Independent review

- Reviewer: `b0001_review`, independent from the B0026 analyst and implementer.
- Verdict: approved with no findings after checking the defect, complete status domain, integer/index safety, allocation bound, all retry consumers, option/empty semantics, GRPCRoute separation, logging rules, and exact scope.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | Resource preparse 28, LB sample 8, and response-filter 2 tests passed; the implementer also ran the exact retry and sample filters. |
| Independent review checks | passed | Resource/Gateway checks, strict resource Clippy, CRD coverage, formatting, no-tracing, English-only, diff, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `29ec67ff9abe66be71f80a06156816a413893ff5`.
