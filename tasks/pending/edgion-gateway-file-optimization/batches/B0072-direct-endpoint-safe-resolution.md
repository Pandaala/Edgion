# B0072 — DirectEndpoint Safe Resolution and Committed Jump Observability

## Scope
- Baseline: `4ad0cf32`
- Pipeline state: implementation independently approved, fully verified, and committed as `65c02ff0`
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/direct_endpoint/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/direct_endpoint/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/direct_endpoint/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, shared resource/config and docs, PluginRuntime/PluginStore, backend validation, session/context APIs, metrics/access-log implementation, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/direct_endpoint/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0071-ctx-set-bounded-transform-execution.md`; and `tasks/pending/edgion-gateway-file-optimization/batches/B0072-direct-endpoint-safe-resolution.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `4ad0cf32`; target blobs are `a39623ceb4e7ddc7c6a1eb6f0c8022c4dc612ed5` for `mod.rs` and `ee547bb7c009a5a8ccd78ea52260d7aedaae3a3b` for `plugin.rs`.
- Baseline: 4 DirectEndpoint, 2 backend-validation, 1 resource DirectEndpoint, and 8 shared UpstreamFallback tests pass.
- Dynamic resolution strings currently classify malformed versus unauthorized targets through text matching and are returned to the client, exposing request-derived values, addresses, plugin identity, or internal validation detail.
- The bracket parser accepts trailing garbage after a valid bracketed IPv6 literal as an implicit no-port address.
- Endpoint formatting and success diagnostics occur before first-write-wins jump ownership is known, so a rejected jump can record a destination that never became effective.
- Decision: introduce payload-free typed resolution categories, strict bracket suffix parsing, fixed public error bodies, and success diagnostics only after `SetJumpOutcome::Set`; also remove the constant per-instance name allocation.
- Expected no-change file: `mod.rs` remains the minimal public module facade.

## Hard-rule boundary
- Preserve raw-missing and regex-miss `onMissing` behavior, malformed `onInvalid` 400 behavior, and unauthorized/unsafe `onInvalid` 403 behavior.
- Preserve Fallback/Reject, Dye outcome/cause, gRPC skip, route/backend-reference requirements, denied-reference filtering, endpoint membership, `validate_endpoint_backend_addr`, TLS/SNI inheritance, and first-write-wins semantics.
- Fixed client bodies are `Missing target endpoint`, `Invalid target endpoint`, and `Target endpoint not allowed`; no request value, target address, dependency error, plugin identity, or validation detail may enter them.
- Preserve bracketed no-port IPv6, bare IPv6, IPv4, and explicit valid ports. Reject any nonempty bracket suffix except a valid `:<u16>`.
- Emit endpoint-sensitive diagnostics and Success Dye only after the jump is accepted. A rejected jump retains only the existing `jump locked, rejected` result.
- Add no tracing, metrics, access-log fields, schema behavior, public APIs, dependencies, integration-test edits, or duplicate parsing/security mechanisms.

## Deferred candidates
- Service-port to EndpointSlice serving-port proof, explicit-port authorization, readiness, numeric comparison, request-path scanning, and generation coherence remain covered by the existing `backend-port-resolution-precompiled-fail-closed.md` task and are forbidden here.
- Regex DFA limits remain an accepted precedent. No duplicate typed-error or strict-bracket finding exists.
- Existing frozen integration coverage exercises successful direct routing, unauthorized target rejection, missing fallback, and invalid-regex status propagation. Colocated tests are sufficient, so no new integration issue is required.

## Implementation
- Complexity: complex; security error-body contract, parser correctness, and canonical request-level observability change together
- Implementer: `b0055_runtime_analysis`; plan-only phase complete and distinct from analyst and reviewers
- Plan reviewer: `b0071_final_code_review`; approved the typed policy mapping, strict parser contract, fixed response boundary, post-commit observability order, and colocated test feasibility
- Exact change: implemented private payload-free resolution categories, strict bracket suffix parsing, fixed 400/403 bodies, a static plugin name, and endpoint diagnostics plus Success Dye only after an accepted jump, with no plan deviation
- Test contract: static identity; supported address forms; strict bracket/port rejection; fixed nonreflective 400/403 bodies; preserved regex-miss and fallback state; jump-lock diagnostics; accepted-jump endpoint logging

## Independent review
- Code reviewer: `b0072_final_code_review`; distinct from analyst, implementer, and plan reviewer
- Code verdict: approved with no blocking findings; confirmed parser/status/policy behavior, nonreflective errors, canonical membership and address-safety order, TLS/SNI preservation, first-write-wins diagnostics, and test isolation
- Deferred adjudication: no new deferred task; existing backend port-resolution task remains the owner of wider membership/generation work

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 4 DirectEndpoint, 2 backend validation, 1 resource, and 8 UpstreamFallback tests |
| Implementation and focused tests | passed | 11 Gateway DirectEndpoint, 2 backend validation, 1 resource DirectEndpoint, and 8 fallback-default tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | no blocking findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: `65c02ff0`
