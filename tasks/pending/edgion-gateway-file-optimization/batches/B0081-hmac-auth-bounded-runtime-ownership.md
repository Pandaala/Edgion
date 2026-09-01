# B0081 — HmacAuth Bounded Runtime Ownership

## Scope
- Original baseline: `332b2163`; user-authorized concurrent merge advanced HEAD to `c15e04d7` without changing either HmacAuth target blob
- Pipeline state: committed with independently approved batch-scoped verification
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/hmac_auth/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs`
- Expected no-change file: `hmac_auth/mod.rs`
- Forbidden files: every existing `edgion-tests/**` path, resources/schema, BodyStorage and PluginSession APIs, condition consumers, documentation, metrics/access-log implementation, and protected user paths
- Exact future commit allow-list: implementation file; refreshed integration-freeze manifest for the user-authorized `c15e04d7` merge; campaign ledger/changelog/main record; B0080 and B0081 records; D0047 record/index/task

## Context and value gate
- Analysis subagent: `b0078_geoip_analysis`; original target blobs are `f0d6223b8e276b828ef67c7ed56a3bc77ef95d52` for `mod.rs` and `a68d27613100e276763a524d29605f109d85ee90` for `plugin.rs`; both remain identical at `c15e04d7`.
- Baseline HmacAuth tests: 19 passed.
- Standalone HmacAuth retained a complete cloned config after its verifier had already cloned resolved credentials, leaving a second long-lived runtime copy of every secret and cold configuration field.
- Canonical signing built one String per signed line, retained them in a Vec, then allocated the joined output. Required header names were also lowercased again per request.
- The no-credentials configuration failure exposed `No credentials configured` in the public 500 body.
- Decision: retain only live runtime policy fields beside the verifier, prepare required names once, assemble byte-identical signing input directly in one output String, and return a generic fixed 500 body.

## Hard-rule boundary
- Preserve cheap authentication before the single canonical request BodyStorage read, physical and plugin caps, body digest semantics, timeout/error mapping, constant-time MAC comparison, timestamp window, signed-header scope, and replay policy.
- Preserve configured header order and duplicates, original spelling in missing-header errors, literal request-target aliases, first-missing failure, path/query/header bytes, and safe ASCII method lowercasing.
- Preserve fixed fail-close status, HMAC challenge, configured delay, response write order, shutdown, and static internal log classification while hiding configuration detail from the client.
- D0035, D0037, and D0038 retain shared header ownership and integration scope. Add no tracing, metrics, access-log fields, cache/replay mechanism, schema behavior, or integration-test edits.

## Deferred candidates
- D0047 records the bilingual `Signature keyId`/header-source/YAML/date documentation drift. It requires documentation only and no new integration coverage.

## Implementation
- Complexity: complex; authentication wire construction required plan-first review
- Implementer: `b0055_runtime_analysis`; plan-only phase completed before editing
- Plan reviewer: `b0081_plan_review`; independently verified the 146-byte canonical input and SHA-256/384/512 vectors, live field set, lookup order, UTF-8 behavior, and fixed response contract
- Exact change: removed the retained full config, prepared enforced headers once, wrote canonical rows into one saturating-capacity String, and changed only the public no-credentials message
- Test contract: runtime field/credential ownership; prepared header order, duplicates, and spelling; exact canonical bytes and three HMAC vectors; complete generic 500 response and log sequence

## Independent review
- Final code reviewer: `b0081_final_code_review`, distinct from analyst, implementer, and plan reviewer
- Verdict: approved with no findings after revalidation at `c15e04d7`; confirmed byte semantics, field consumers, secret-copy claim, response contract, BodyStorage/time/constant-time boundaries, and no integration diff
- Deferred adjudication: D0047 is nonduplicate; existing D0035/D0037/D0038 remain unchanged; no integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Concurrent revalidation | passed | HmacAuth target blobs unchanged at `c15e04d7`; integration-freeze manifest refreshed to that user-authorized committed baseline |
| Focused tests | passed | 23 HmacAuth, 36 condition evaluator, 15 RequestRestriction, 15 RequestAccessPolicy, 15 resource, and 34 PluginRuntime tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | no findings; Gateway format/check/no-tracing and diff checks rerun |
| Gateway-scoped gates | passed | Gateway format, all-target check, and all-target Clippy passed; Clippy emitted existing warnings only |
| Repository guards | passed | agent docs, SSA, metrics, Pingora source, and gateway no-tracing passed |
| Full workspace gates | external failure | format, check, and Clippy each stop on the mismatched delimiter in `edgion-tests/src/client/suites/edgion_plugins/guardrail/guardrail.rs` committed by user-authorized merge `c15e04d7`; B0081 does not modify that path |
| Frozen integration paths | passed | no B0081 integration diff after rebasing the external committed baseline |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row
- Commit: `2ac3cc2b`
