# B0075 — FaultInjection Deterministic Decisions and Duplicate Response Headers

## Scope
- Baseline: `42b25fca`
- Pipeline state: implementation independently approved, fully verified, and committed as `a542c7e0`
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/fault_injection/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/fault_injection/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/fault_injection/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, shared resource/config and docs, PluginRuntime response writer, session/context APIs, condition engine, metrics/access-log implementation, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/fault_injection/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0074-dynamic-internal-upstream-borrowed-selection.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0075-fault-injection-deterministic-response-headers.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/D0046-final-http-status-validation-local-replies.md`; `tasks/pending/edgion-gateway-file-optimization/deferred/INDEX.md`; and `tasks/todo/use-canonical-final-http-status-validation-for-local-replies.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against local and remote `42b25fca`; target blobs are `243ce955a3ded95d4db9e1f2419675f5f7db7aeb` for `mod.rs` and `39bb5d344431d2214a736dea56917584ecd2a015` for `plugin.rs`.
- Baseline: 5 Gateway FaultInjection, 17 resource FaultInjection, 2 generic Probability, and 34 PluginRuntime tests pass.
- The constant plugin name is allocated per instance, and ratio endpoints consume RNG even though their outcomes are deterministic.
- `AbortConfig.headers` promises order preservation and duplicate support, but the plugin uses response-header replacement. Later values overwrite earlier same-name headers, including multiple `Set-Cookie` values.
- Decision: return a static name, keep `decide` private, short-circuit ratios zero and one without RNG, and append configured response headers in order while retaining the dedicated Content-Type replacement.
- Expected no-change file: `mod.rs` remains the accurate minimal module facade.

## Hard-rule boundary
- Preserve outer entry conditions, independent delay/abort decisions, exactly one RNG sample for intermediate ratios, inclusive delay ranges, the 30-second cap, and dropping RNG before any await.
- Preserve delay before abort, cancellable async sleep, existing fixed PluginLog text, body ownership/cloning, and request-stage `ErrResponse` response ownership through PluginRuntime.
- Content-Type remains the dedicated replacement header. Additional configured headers use canonical append semantics in configuration order so duplicates survive.
- Preserve all response statuses/bodies, failure policy, probability behavior outside ratio endpoints, no-Dye capability, and immutable hot reload.
- Add no tracing, metrics, access-log fields, schema behavior, public APIs, dependencies, integration-test edits, response self-writing, or `ErrTerminateRequest`.

## Deferred candidates
- D0046 is `needs-implementation`: FaultInjection, Mock, RequestAccessPolicy, RateLimit, RateLimitRedis, IpRestriction, and RequestRestriction accept 1xx request-stage reply statuses, but the canonical final-response gate accepts only 200 through 599. Shared validation and schema ranges must reuse the canonical predicate across both runtime-owned `ErrResponse` and self-written reply paths; do not change the runtime writer in B0075.
- GEP-2257 explicitly retains these integer duration fields. Generic Probability semantics remain separately owned and are not duplicated here.
- Existing frozen integration coverage has 8 cases for delay, abort headers/body, combined ordering, ratio zero, entry conditions, 204, and access-log diagnostics. Colocated tests are sufficient, so no new integration issue is required.

## Implementation
- Complexity: simple; one private decision helper and canonical response-header operation
- Implementer: `b0055_runtime_analysis`; distinct from analyst and final reviewer
- Exact change: returned a static name, made endpoint ratios deterministic without RNG calls, and appended configured response headers in order after the dedicated Content-Type replacement
- Test contract: static name identity; zero/one ratios consume no RNG; intermediate ratio distribution retained; deterministic abort with Content-Type replacement and duplicate appended headers; existing fixed/range boundaries

## Independent review
- Code reviewer: `b0075_final_code_review`; distinct from analyst and implementer
- Code verdict: approved with no code findings; confirmed RNG call counts, independent decisions, pre-await handle drop, response ownership, header operation order, duplicate preservation, and deterministic tests
- Deferred adjudication: D0046 `needs-implementation` across seven plugins and both local-response ownership shapes; shared validation/schema work, no runtime-writer or integration change

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | local and remote HEAD agree; protected external paths unchanged |
| Baseline focused tests | passed | 5 Gateway, 17 resource, 2 Probability, and 34 PluginRuntime tests |
| Implementation and focused tests | passed | 7 Gateway, 17 resource, 2 Probability, and 34 PluginRuntime tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | code approved; deferred scope corrected and re-approved |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: `a542c7e0`
