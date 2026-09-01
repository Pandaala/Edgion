# B0085 — JwtAuth Claim Enforcement and Generation Preparation

## Scope
- Baseline: `b9e3223a`
- Pipeline state: independently approved and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: all 15 tracked files under `edgion-gateway/src/plugins/http/jwt_auth/`
- Exact implementation files: `credentials.rs`, `plugin.rs`, `verify.rs`, `tests/claim_validation.rs`, and `tests/failure_response.rs`
- Forbidden files: resources, Controller, shared auth/session/runtime, documentation, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- Analysis found that `nbf` was not enabled, a configured nonempty audience did not require an `aud` claim, and valid fractional NumericDate values bypassed `maximumExpiration`.
- Every request also rebuilt Validation and its audience set, while each runtime generation retained a duplicate resolved credential map after preparing decoding keys.
- Decision: consume resolved material into immutable generation-owned keys and Validation objects, enforce the documented claims, and preserve the exact public authentication contract.

## Hard-rule boundary
- Preserve Header > opt-in Query > Cookie priority, missing-only anonymous access, issuer and group-scoped key selection, algorithm-confusion defense, shared auth delay, fixed generic 401, challenge distinction, static redacted denial logs, and success mutation order.
- Add no failure-policy knob, response buffering, tracing, metric, access-log field, schema change, or integration-test edit.

## Implementation and review
- Complexity: complex; implementer supplied a no-edit plan and an independent reviewer approved the corrected NumericDate and Tokio-time details before implementation
- Analyst: `b0084_plan_review`; implementer: `b0055_runtime_analysis`; plan/final reviewer: `b0084_final_code_review`
- Exact change: prepared and reused per-group decoding keys and Validation, removed retained raw resolved groups, enabled optional `nbf`, required `aud` only for nonempty configured audiences, and enforced fractional expiration caps with pinned jsonwebtoken semantics
- Final correction: removed the remaining successful-request Arc clone and made `VerifyOk` Debug fully opaque; independent re-review approved the borrowed generation lifetime and sentinel regression
- Deferred adjudication: D0052 tracks deterministic shared claim-to-header projection. Existing integration coverage remains frozen and no new issue is needed.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 45 Gateway JwtAuth, 10 resource JwtAuth, 3 Controller JWT resolver, and 289 PluginRuntime tests |
| Independent reviews | passed | separate analysis plus corrected plan and final code review; paused-time test also passed 20 consecutive reviewer runs |
| Full workspace gates | passed | format check, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, SSA, metrics, Pingora source, gateway no-tracing, diff, and frozen paths |
| Frozen integration paths | passed | no `edgion-tests/**` worktree diff relative to the user-authorized manifest |

## Handoff
- Ledger/changelog updated: yes; 5 changed and 10 examined-no-change owned rows
- Commit: pending
