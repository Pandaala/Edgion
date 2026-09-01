# B0083 — JsonSchemaValidation Canonical Body Errors

## Scope
- Baseline: `e153765f`
- Pipeline state: independently approved and batch-scoped verification complete; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/json_schema_validation/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/json_schema_validation/plugin.rs`
- Expected no-change file: `json_schema_validation/mod.rs`
- Forbidden files: every existing `edgion-tests/**` path, BodyStorage/body-gate APIs, resources/schema compiler, access-log/metrics implementation, documentation, and protected user paths
- Exact future commit allow-list: implementation file; campaign ledger/changelog/main record; B0082 and B0083 batch records

## Context and value gate
- Analysis subagent: `b0078_geoip_analysis`; target blobs are `99e4d6c3b167935600fd68c65b0e57ce18569634` for `mod.rs` and `856307d79077526e424c62d2cc6239e90928b4f0` for `plugin.rs`.
- Baseline tests: 11 Gateway, 22 resource, 4 strict-parser, 2 default-limit, and 28 canonical body-gate tests passed.
- The plugin duplicated the complete eight-variant `BodyReadError` to `EdgionErrorCode` mapping already owned by the canonical request body gate.
- Decision: delete the local mapping and route both existing error sites through `record_body_read_error_status`.

## Hard-rule boundary
- Preserve the single canonical request BodyStorage, physical and configured logical caps, availability/read order, strict JSON parsing, schema validation, fixed fail-close responses, static logs, and status recording.
- Add no response buffering, tracing, metrics, access-log fields, schema behavior, dependency, or integration-test edits.

## Implementation and review
- Complexity: simple; no plan-first review required
- Implementer: `b0055_runtime_analysis`, distinct from analyst and reviewer
- Exact change: removed the local helper and imported the canonical body-gate recorder; `BodyReadError` remains only in the test module
- Code reviewer: `b0083_final_review`; approved exact equivalence for all eight variants, both call positions, test-only imports, body/status/log behavior, and integration scope
- Deferred adjudication: none; existing frozen coverage is sufficient

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 11 Gateway, 22 resource, 4 strict-parser, 2 default-limit, and 28 body-gate tests |
| Independent review | passed | reviewer reran narrower JSON-schema and body-gate filters plus no-tracing/diff/frozen checks |
| Gateway checks | passed | implementation format, all-target check, Clippy, and no-tracing passed |
| Repository guards | passed | agent docs, SSA, metrics, Pingora source, gateway no-tracing, and frozen paths passed |
| Full workspace gates | external failure | recorded `c15e04d7` integration parse error remains outside B0083 scope |
| Frozen integration paths | passed | refreshed external baseline unchanged by B0083 |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row
- Commit: pending
