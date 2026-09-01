# B0076 — FormJsonTransform Owned JSON Value Encoding

## Scope
- Baseline: `a542c7e0`
- Pipeline state: implementation independently approved and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/form_json_transform/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/form_json_transform/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/form_json_transform/mod.rs`
- Allowed tests: existing colocated tests in the implementation file
- Forbidden files: every existing `edgion-tests/**` path, shared resource/config and docs, BodyStorage/request body state and capture gate, session/context/framing APIs, shared parser/CappedWriter, metrics/access-log implementation, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/form_json_transform/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0075-fault-injection-deterministic-response-headers.md`; and `tasks/pending/edgion-gateway-file-optimization/batches/B0076-form-json-transform-owned-value-encoding.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `a542c7e0`; target blobs are `2cd8f3afe437637cc660866a5b19a22e02a6a9e0` for `mod.rs` and `5842fd8526fea4b667297c4e24d3124e79d560fc` for `plugin.rs`.
- Baseline: 8 Gateway FormJsonTransform, 10 combined plugin/runtime, 4 resource, 4 shared media, 1 CappedWriter, and 3 RequestBodyState ownership/framing tests pass.
- `json_to_form` already owns every `Value` moved from the parsed object or array, but `write_json_pair(&Value)` clones every string again before percent encoding.
- At configured entry/value limits, avoiding 256 separate 16-KiB string copies removes about 4 MiB of transient allocation while preserving exact output.
- Decision: pass owned `Value` objects into the private encoder and move the inner string; retain all number, boolean, ordering, encoding, and cap behavior.
- Expected no-change file: `mod.rs` remains the accurate minimal facade.

## Hard-rule boundary
- Preserve the single canonical request `BodyStorage`, capture/materialization order, input/output/body caps, structural depth/node and pair/key/value limits, strict JSON/form parsing, duplicate-key behavior, and fixed fail-close responses.
- Preserve explicit member sorting as a wire-order contract and the zero-copy `Bytes::from(Vec<u8>)` transfer.
- Preserve the original body until Content-Type publication succeeds, rollback behavior, lazy bodyless ownership, immutable overwrite generations, retry replay, and runtime-owned Content-Length/Transfer-Encoding framing.
- Preserve same-direction rejection, canonical Content-Type mutation, static PluginLog text, hot reload, and `requires_materialized_request_body = false`.
- Add no second body cache, response aggregation, tracing, metrics, access-log fields, schema/failure-policy behavior, dependencies, integration-test edits, or allocator-sensitive tests.

## Deferred candidates
- H1 bounded capture copying remains an accepted closed tradeoff. No duplicate or new wider finding exists.
- Existing frozen integration coverage exercises memory/disk bodies, strict parsing, limits, content type/framing, retries, chaining, and hot reload. Existing colocated tests are sufficient, so no new integration issue is required.

## Implementation
- Complexity: simple; private owned-value transfer at two already-owned call sites
- Implementer: `b0055_runtime_analysis`; distinct from analyst and final reviewer
- Exact change: changed the private encoder to consume `Value`, passed both existing owned call sites by value, and moved the inner `String` directly into percent encoding
- Test contract: existing exact WHATWG encoding/sorted-key test covers string, array, boolean, number, empty value, and wire order; all focused suites must remain green

## Independent review
- Code reviewer: `b0076_final_code_review`; distinct from analyst and implementer
- Code verdict: approved; confirmed ownership at both call sites, exact scalar/array encoding and limits, and unchanged body cache, rollback, publication, and framing paths
- Deferred adjudication: no new deferred or integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | protected external paths unchanged |
| Baseline focused tests | passed | 8 plugin, 10 plugin/runtime, 4 resource, 4 media, 1 writer, and 3 body ownership/framing tests |
| Implementation and focused tests | passed | 8 plugin, 10 plugin/runtime, 4 resource, 4 media, 1 CappedWriter, and 3 body ownership/framing tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | approved with no findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row, all verification complete
- Commit: pending
