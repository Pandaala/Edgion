# B0086 — JwtAuth Documentation Alignment

## Scope
- Baseline: `0248159d`
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Anchor coverage: no new rows; this supporting batch documents the completed B0085 JwtAuth audit
- Exact support files: the English and Chinese JwtAuth user guides and developer guides, plus JwtAuth resource Rustdoc
- Forbidden files: Gateway runtime and tests, Controller code, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- The user guides still promised an implicit success identity header, allowed invalid tokens through anonymous fallback, and generated examples with a JOSE header key selector that the runtime never reads.
- The developer guides still described a stub, legacy Secret fields, obsolete code paths, and several implemented features as future work.
- Decision: align both languages with the reviewed B0085 runtime and Controller contract without changing behavior or integration tests.

## Hard-rule boundary
- Preserve canonical access-log ownership, sensitive `sub` masking, static redacted denial classifications, the fixed generic 401 response, and explicit claim-to-header projection.
- Distinguish the payload `keyClaimName` selector from JOSE `kid`, group-level inheritance from the effective audience policy, and Kubernetes Secret transport decoding from optional inner Base64 key decoding.
- Add no feature, compatibility statement, runtime code, test mutation, or new integration issue.

## Implementation and review
- Complexity: moderate; implementation followed the independently analyzed B0085 documentation split
- Analyst: `b0084_plan_review`; implementer: `b0055_runtime_analysis`; final reviewer: `b0084_final_code_review`
- Exact change: documented issuer groups, Controller Secret resolution and generation preparation, supported algorithms, token-source priority, required `exp`, optional validated `nbf`, audience three-state merging, maximum expiration, missing-only anonymous access, explicit header projection, optional verified-claims context storage, generic challenges, realm, and OIDC/JWKS boundaries
- Review correction: removed remaining `kid` terminology for the payload selector, clarified effective audience semantics and double-layer Base64 handling, added the omitted realm field and exact challenges, and documented the missing-`sub` log placeholder
- Final verdict: approved after correction; English and Chinese semantics are symmetric and all claims match current resource, Controller, and Gateway behavior
- Deferred adjudication: none. Existing frozen integration coverage remains the validation floor and no new integration issue is required.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 10 resource JwtAuth and 45 Gateway JwtAuth tests |
| Independent review | passed | initial four documentation findings corrected and independently re-reviewed |
| Full workspace gates | passed | format check, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, English-only, SSA, metrics, Pingora source, gateway no-tracing, and diff checks |
| Frozen integration paths | passed | no `edgion-tests/**` worktree diff and no integration issue needed |

## Handoff
- Ledger unchanged (support-only); changelog updated: yes; no anchor coverage count changed
- Commit: `520a38ee`
