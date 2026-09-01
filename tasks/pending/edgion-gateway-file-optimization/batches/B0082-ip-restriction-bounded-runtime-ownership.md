# B0082 — IpRestriction Bounded Runtime Ownership

## Scope
- Baseline: `2ac3cc2b`
- Pipeline state: independently approved and batch-scoped verification complete; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/ip_restriction/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs`
- Expected no-change file: `ip_restriction/mod.rs`
- Forbidden files: every existing `edgion-tests/**` path, shared resources/matcher/store APIs, stream/TLS/GCIR consumers, access-log/metrics implementation, documentation, and protected user paths
- Exact future commit allow-list: implementation file; campaign ledger/changelog/main record; B0081 and B0082 records; D0048/D0049 records, index, and tasks

## Context and value gate
- Analysis subagent: `b0078_geoip_analysis`; target blobs are `c93c4dc233f5ef7b5fa0bc64c5a358368a4ef220` for `mod.rs` and `4b73ed1c650a01b81e6c0ff82d3ad213af757e14` for `plugin.rs`.
- Baseline tests: 8 Gateway and 30 resource IpRestriction tests passed.
- The plugin retained original groups, CIDR strings, descriptions, refs, and message beside compiled matchers and flattened reference keys, duplicating long-lived ACL policy memory.
- Each decision created matcher-reference Vecs before the resource helper created combined Vecs again. Denial also rebuilt and serialized fixed JSON per rejected request.
- Decision: retain compiled runtime fields only, use bounded matcher-borrow collections with the canonical evaluator, prebuild denial bytes, and format group logs without a temporary String.

## Hard-rule boundary
- Preserve DirectPeerIp/RemoteIp selection, no-IP behavior, allow/deny plus `defaultAction`, deny-first and inline-before-reference ordering, partial-good CIDRs, live references, denied/missing reference policy, and same-key reload.
- Preserve initialization fail-close, configured status/message, exact JSON bytes, response write/body/shutdown order, skipped-entry metric, and bounded static logs.
- D0046 retains final-status validation. Add no ad-hoc matching/extraction, tracing, metrics, access-log fields, caches, schemas, dependencies, or integration-test edits.

## Implementation
- Complexity: complex; authorization hot-path ownership required plan-first review
- Implementer: `b0055_runtime_analysis`; plan-only phase completed before editing
- Plan reviewer: `b0082_plan_review`; approved field-move ordering, distinct init response, matcher ordering and lifetimes, canonical evaluation, and deferred separation
- Exact change: moved compiled fields from the validated temporary config, froze whitelist intent, prebuilt denial Bytes, combined inline/reference matcher borrows in SmallVec, used static identity and bounded log formatting, and kept every Arc guard before the first await
- Test contract: static name; fixed init body; exact custom/default denial bytes; partial-good match; refs-only default fallback; referenced deny precedence over inline allow; denied allow-ref whitelist intent; same-key reload

## Independent review
- Final code reviewer: `b0082_final_code_review`, distinct from analyst, implementer, and plan reviewer
- Initial verdict: one test-coverage correction required because a refs-only default-fallback test had been repurposed
- Correction verdict: approved after restoring the original `allow:default` case and adding a separate referenced-deny versus inline-allow precedence test; production code had no findings
- Deferred adjudication: D0048 and D0049 are nonduplicates; D0046 remains unchanged; no integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 12 Gateway, 30 resource, 42 canonical IP helper, 8 store, and 34 PluginRuntime tests |
| Gateway checks | passed | format, all-target compile and Clippy; existing warnings only |
| Independent review | passed after correction | restored refs-only/default fallback coverage and separated precedence coverage |
| Repository guards | passed | agent docs, SSA, metrics, Pingora source, gateway no-tracing, and frozen paths passed |
| Full workspace gates | external failure | the previously recorded `c15e04d7` integration parse error remains outside B0082 scope; B0082 Gateway-scoped gates pass |
| Frozen integration paths | passed | refreshed external baseline unchanged by B0082 |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row
- Commit: pending
