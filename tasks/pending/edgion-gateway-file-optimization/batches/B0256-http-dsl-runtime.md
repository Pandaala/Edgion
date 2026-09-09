# B0256 — HTTP DSL Runtime

## Scope

- Pipeline state: in-progress; refreshed analysis passed the value gate after B0255.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `efd98773`.
- Target: `edgion-gateway/src/plugins/http/dsl/runtime.rs`.
- Target blob: `fec56ae891bd43b996b90b36657bd87a43263d08`.
- Read scope includes the complete runtime construction/execution lifecycle, plugin/VM/builtin consumers, resource inputs, tests, and directly owning architecture/review knowledge.
- The prior B0254 reviewer identified `skills/01-architecture/02-gateway/dsl-engine.md` as a possible exact architecture-support path for construction-time wording, the `rate_limits` excerpt, and the body-scanner path. The analyst must independently confirm scope and value.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, documentation-only.
- `DslRuntime` is built once per plugin/config generation by `resolve_parts`, stored in the shared plugin, and borrowed per execution. The target incorrectly describes it as per-request and constructed at request time.
- Owning knowledge also contains directly related drift: removed body-scanner paths, a missing `rate_limits` field, obsolete one-client/two-client HTTP descriptions, direct `.no_proxy()` ownership instead of the shared outbound factory, and a one-dimensional cache key instead of `(block_private_ips, follow_redirects)`.
- Exact technical allow-list: the target plus `dsl-engine.md`, `dsl-engine-security-model.md`, and the DSL no-env-proxy regression guard. Preserve all runtime/schema/protocol/SSRF/redirect/Redis/body/logging/metrics/integration behavior.
- No focused runtime test is justified; documentation validation/search plus campaign gates are required. D0132 already fully owns the separate scalar-limit issue.

## Implementation and Review

- Corrected the target to describe immutable per-plugin/config-generation construction and request-time borrowing rather than per-request construction.
- Updated owning architecture/security/SSRF knowledge for the MiB unit, current request-body scanner paths, `rate_limits`, four `(block_private_ips, follow_redirects)` client variants, canonical outbound-client construction, proxy disablement, and current sibling paths.
- Independent final review: LGTM, no Must Fix, Should Fix, or new deferred finding. It traced plugin-generation ownership, VM state, client selection, redirect/egress behavior, Redis live lookup, and all changed links/text.
- D0132 fully owns the separate scalar-range defect; no duplicate task was created.

## Verification

- Format check, agent-doc validation (1,276 files), stale-path/phrase search, and diff/scope checks passed.
- Coordinator: `cargo fmt --all`; locked workspace check; locked workspace Clippy (exit 0 with pre-existing warnings); agent docs; SSA force; metrics inventory; Pingora source policy; Gateway no-tracing; English-only; diff check; and frozen paths all passed.
- Frozen integration manifest: 3,231 paths unchanged.
