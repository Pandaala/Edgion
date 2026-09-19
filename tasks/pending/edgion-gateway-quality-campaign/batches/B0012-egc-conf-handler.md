# B0012 — EGC Configuration Handler

## Scope

- Baseline: `b3274df3ab8529485b7df151aad9f93791895e95`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`
- Allowed code files: the anchor plus `edgion-gateway/src/config/edgion_gateway/mod.rs`, `edgion-gateway/src/config/mod.rs`, and `edgion-gateway/src/backends/mod.rs`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the four code files above and B0011/B0012 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0012 analysis turn.
- Confirmed defect: deterministic EGC DNS-default winner selection already occurs during snapshot publication, but resolver request paths still clone the full DNS config and ExternalName requests reparse its human-readable TTL.
- Decision: `change` (simple). Pre-resolve the canonical candidate and TTL into one immutable snapshot object and make consumers pin it with an `Arc`.
- Preserved semantics: Service to Gateway to EGC to system precedence, dangling-ref system fallback, live LinkSys lookup, deterministic lowest-key EGC selection, full/partial/delete updates, invalid and zero TTL behavior, request-body/L4 snapshot policy, schema and transport behavior.
- Freshness revalidation: valid at `b3274df3a`; B0011 changed only CLI Pingora integration and campaign records.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0012 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: added an immutable resolved DNS-default value containing the canonical resolver candidate and parsed TTL, published it with the owning EGC snapshot, and changed resolver consumers to clone its `Arc` instead of the full config.
- Tests: expanded local snapshot coverage for deterministic winner selection, candidate and TTL resolution, old-generation pinning, system-candidate optionality, invalid fallback, zero TTL, and deletion.

## Independent review

- Reviewer: `b0001_review`, independent from the B0012 analyst and implementer.
- Verdict: approved with no findings. The reviewer confirmed the remaining candidate clone is the minimum ownership cost under the canonical resolver API, while full config cloning and TTL parsing are removed from request paths.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | 31 handler, 18 DNS-cache, and 12 resolver-selection tests plus Gateway all-target check, formatting, no-tracing, English-only, agent-doc, diff, and frozen checks passed. |
| Independent review checks | passed | Focused snapshot and resolver suites, Gateway library check/clippy, formatting, diff, no-tracing, metrics-inventory, and agent-doc checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `936032962e3e46e90a0cdf9936f5cd415ab804db`.
