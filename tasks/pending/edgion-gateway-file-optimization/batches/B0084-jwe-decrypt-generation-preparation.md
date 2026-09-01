# B0084 — JWE Decrypt Generation Preparation

## Scope
- Baseline: `0a95362a`; the two owned blobs were unchanged from the queued `1967292a` analysis baseline
- Pipeline state: independently approved and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/jwe_decrypt/{mod.rs,plugin.rs}`
- Supporting documentation: `docs/{en,zh-CN}/user-guide/edgion-plugins/jwe-decrypt.md`
- Forbidden files: every existing `edgion-tests/**` path, shared resource validation, Controller resolution, plugin runtime framework, and protected user paths

## Context and value gate
- Analysis found a request-time lock, key clone, and cipher rebuild even though the Controller already publishes immutable resolved credentials per plugin generation.
- Decision: consume resolved key material during construction, retain one immutable cipher per generation, reject unsupported protected `zip`/`crit` parameters, and preserve raw protected-segment AAD.
- Invalid resolved credential behavior at the standard `compile_plugins` boundary remains the current generic invalid-plugin 500 response and is documented and tested as current behavior, not endorsed policy.

## Hard-rule boundary
- Preserve the compact `dir`/`A256GCM` profile, token cap, algorithm allowlist, failure precedence, static redacted logs, configured auth delay, Bearer challenge, and success mutation order.
- Add no request or response body buffering, tracing, metrics, schema change, dependency, or integration-test edit.

## Implementation and review
- Complexity: complex; plan-first review completed before implementation
- Analyst: `b0078_geoip_analysis`; implementer: `b0055_runtime_analysis`; plan reviewer: `b0084_plan_review`; final reviewer: `b0084_final_code_review`
- Exact change: replaced lazy locked key caching with a prepared decryptor, removed the retained resolved credential, rejected `zip`/`crit` by presence, and added generation-isolation, raw-AAD, failure-order, and real compile-boundary regressions
- Final verdict: approved with no findings after correcting the docs and tests to reflect the actual `compile_plugins` invalid-configuration path
- Deferred adjudication: D0050 tracks strict protected-header JSON parsing; D0051 tracks malformed resolved-credential auth failure policy. No integration issue is required.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 11 Gateway JWE, 5 resource JWE, 1 Controller Secret-resolution, 34 reviewer PluginRuntime, and 289 coordinator runtime-filtered tests |
| Independent reviews | passed | separate plan and final code reviewers approved the corrected three-file diff |
| Full workspace gates | passed | format check, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, SSA, metrics, Pingora source, gateway no-tracing, diff, and frozen paths |
| Frozen integration paths | passed | rebaselined to user-authorized external HEAD `0a95362a`; B0084 changed no integration file |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row
- Commit: pending
