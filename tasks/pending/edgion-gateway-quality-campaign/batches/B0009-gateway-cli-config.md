# B0009 — Gateway CLI Config

## Scope

- Baseline: `c6cbf549cdd07c85901001e334f8600ea59e7846`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/cli/config.rs`
- Allowed code and documentation files: `edgion-gateway/src/cli/config.rs`, `edgion-resources/src/observe.rs`, `edgion-gateway/src/observe/access_log/entry.rs`, `skills/02-features/02-config/01-gateway-config.md`, `docs/en/ops-guide/observability/access-log.md`, `docs/zh-CN/ops-guide/observability/access-log.md`, `docs/en/path-normalization.md`, `docs/zh-CN/path-normalization.md`, `docs/zh-CN/ops-guide/configuration/gateway-config.md`, and `skills/04-review/cpu-memory/intentional-design/access-log-request-uri-include-raw-uri-alloc.md`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the files above, B0009/D0005 campaign records, `tasks/todo/strict-gateway-process-config-unknown-fields.md`, and campaign ledger/main/changelog/deferred-index records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0009 analysis turn, including freshness validation against the current HEAD.
- Confirmed defect: `AccessLogOptions` uses canonical camelCase serde names, but source and authoritative documentation examples use `include_raw_uri`. Unknown fields are accepted, so those examples load without error while leaving raw-URI capture disabled.
- Decision: `change` (simple). Preserve `includeRawUri` as the sole configuration key, reject misspellings on the closed access-log option object, add clean-and-typo serde coverage, and correct config-facing examples without renaming Rust identifiers.
- Preserved semantics: request/access-log behavior for valid config, canonical access-log ownership, URI redaction/opt-in behavior, observability surfaces, FIPS/TLS behavior, schema and transport behavior.
- Frozen integration evidence: the existing integration fixture already uses `includeRawUri`; it remains read-only.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0009 implementation turn.
- Complexity: simple; no plan-review stage required.
- Actual change: `AccessLogOptions` now rejects unknown fields while retaining
  `includeRawUri` as its only configuration key. Owning-resource and root
  Gateway YAML tests cover canonical success and snake_case typo rejection.
- Documentation and knowledge: all approved config-facing examples now use
  `access_log_options.includeRawUri`; Rust identifiers remain unchanged.

## Independent review

- Reviewer: `b0001_review`, independent from the B0009 analyst and implementer.
- Initial verdict: implementation approved, but terminalization rejected
  because the coordinator-owned D0005 task named a nonexistent
  `accessLog.includeRawUri` path.
- Final verdict: approved after correcting D0005 to the canonical
  `access_log_options.includeRawUri` path. No implementation finding remains.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Baseline Gateway config suite | passed | 48 tests passed during analysis. |
| Baseline resources observe suite | passed | 2 tests passed during analysis. |
| Frozen integration paths | passed | Existing integration files remained unchanged during analysis. |
| Focused implementation checks | passed | 3 resources observe and 49 Gateway config tests plus formatting, agent-doc, no-tracing, English-only, and diff checks passed. |
| Independent review checks | passed | Focused resource/config tests, formatting, diff, agent-doc, no-tracing, metrics-inventory, English-only, scope, and canonical-path checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `6f93c8c92882031b2d5d288811580071f28f45d7`.
