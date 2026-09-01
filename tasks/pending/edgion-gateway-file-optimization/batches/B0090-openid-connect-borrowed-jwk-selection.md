# B0090 — OpenID Connect Borrowed JWK Selection

## Scope
- Baseline: `ea6c94ba`; all three target blobs were unchanged when the batch started
- Pipeline state: independently approved and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned and exact implementation files: `edgion-gateway/src/plugins/http/openid_connect/{jwt_common.rs,token_verify.rs,tests.rs}`
- Forbidden files: every other OpenID Connect file, resources, Controller, documentation, shared clients and runtime/session APIs, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- JWK selection returned an owned `Jwk`, deeply cloning the selected RSA, EC, OKP, or symmetric key strings for every JWT and callback ID-token verification.
- The caller immediately borrowed that clone to construct an owned `DecodingKey` before its next asynchronous operation, so the cloned JWK lifetime had no semantic value.
- Decision: return a reference to the source `JwkSet` entry and construct the decoding key while that set is alive.

## Hard-rule boundary
- Preserve header decoding, algorithm policy, first matching `kid`, no-`kid` first-compatible iteration order, same-`kid` algorithm mismatch behavior, and the single forced refresh only for an initial missing `kid`.
- Preserve all fetch, refresh, selection, decoding, discovery, claim, scope, 401/502, PluginLog, Dye, SAN metric, and credential-hiding behavior.
- Never retain a JWK reference across an await, log or format key material, add tracing or metrics, or modify existing integration tests.

## Implementation and review
- Complexity: moderate authentication hot-path change; implementation started only after an independent plan review
- Analyst: `b0084_final_code_review`; implementer: `b0055_runtime_analysis`; plan reviewer: `b0084_plan_review`; final reviewer: `b0088_new_final_review`
- Exact change: `select_jwk` now returns the matching source-set reference; both the initial and forced-refresh branches immediately convert that reference into an owned `DecodingKey` before any later await
- Regression coverage: exact source pointer for `kid` selection, no-`kid` compatible selection, multiple compatible keys retaining first-entry order, and a same-`kid` algorithm mismatch not scanning later duplicates
- Final verdict: approved with no findings; borrowed lifetimes, refresh branches, error mapping, observability, and the exact three-file boundary were preserved
- Deferred adjudication: D0056 owns the cross-cutting OIDC egress-client design, and D0057 owns its future new integration coverage. Neither is part of the JWK ownership change.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 5 JWK-selection, 100 complete OIDC-module, and 41 PluginRuntime tests |
| Independent reviews | passed | plan and final code reviews approved with no remaining issue |
| Full workspace gates | passed | format, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, English-only, SSA, metrics, Pingora source, gateway no-tracing, diff, scope, and frozen paths |
| Frozen integration paths | passed | no existing integration file changed; future egress coverage is a separate blocked task |

## Handoff
- Ledger/changelog updated: yes; all 3 owned rows changed
- Commit: pending
