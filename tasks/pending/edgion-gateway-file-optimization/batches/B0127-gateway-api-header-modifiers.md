# B0127 — Gateway API Header Modifiers

## Scope

- Baseline: `da2567094703a53ad928698be10388b450e39e43`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_header_modifier.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/response_header_modifier.rs`
- Baseline blobs: `ba64470dc380d69dee6fc33cbae853cfcefd5e9f` and `9b741e497b3156830f69cd2ab86fc9d84e4da972`
- Exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_header_modifier.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/response_header_modifier.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/mod.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/header_modifier.rs` (new)
  - `docs/en/user-guide/http-route/filters/gateway-api/request-header-modifier.md`
  - `docs/zh-CN/user-guide/http-route/filters/gateway-api/request-header-modifier.md`
  - `docs/en/user-guide/http-route/filters/gateway-api/response-header-modifier.md`
  - `docs/zh-CN/user-guide/http-route/filters/gateway-api/response-header-modifier.md`
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read both header-modifier adapters, shared schema validation, compilation/publication, request and response mutation semantics, tests, docs, and relevant review knowledge.
- Preserve ordered add/set/remove behavior, header safety and sensitivity, immutable publication, canonical diagnostics/access-log ownership, observability restraint, and streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change both owned files. Gateway API v1.5 treats case-insensitively equivalent names within each set/add/remove list as first-entry-wins, but the runtime executes every entry, so later set values win and duplicate add values append. Normalize once during immutable construction while preserving the first spelling/value/order and the cross-operation `set → add → remove` sequence.
- Proposed scope: both owned adapters; `gateway_api_filters/mod.rs`; new private `gateway_api_filters/header_modifier.rs`; and paired EN/ZH request-header-modifier docs. The shared plan consumes HTTP/GRPC configs, retains private immutable slices, and performs no per-request folding or allocation.
- Complexity: complex and plan-first. Although normalization is locally simple, it changes client-visible HTTP/GRPC request/response behavior, introduces a shared runtime module, and updates bilingual contracts.
- Preserved behavior: HTTP/GRPC parity, append of pre-existing field lines, cross-list removal precedence, current runtime parse/error diagnostics pending shared validation, response streaming, no sensitive configured values in logs, no tracing/metrics, and no plugin-local framing policy.
- Deferred findings: [`D0090`](../deferred/D0090-gateway-api-header-modifier-validation-preparation-parity.md) owns native/EdgionPlugins header-modifier validation and immutable typed preparation parity. [`D0091`](../deferred/D0091-proxied-request-header-mutation-framing-policy.md) owns the confirmed proxied `Content-Length`/body divergence and shared request-framing policy. D0068 remains response-only and is not duplicated. No integration issue is currently needed.
- Freshness: promoted at `2cae2cfc`; both owned blobs and the module/request-doc support blobs still match the `da256709` analysis baseline, while the proposed shared module remains absent. The response-doc scope correction was independently revalidated at blobs `15610be68fa21a50d98f0ee184a5c74437efdc0d` and `e4c976dc70765cb45376a81ca82eab7bb6e6d4ac`. B0126 touched none of these paths or their investigated header-mutation call chains.
- Scope amendment: both response guides directly contradicted the approved first-equivalent-entry correction through duplicate case-equivalent `Vary` additions. Independent analysis approved adding exactly those two paths and found no other directly stale documentation; the paired correction preserves upstream-value append semantics and bilingual parity.
- Approved plan: add one private construction-time `HeaderModifierPlan`; normalize set/add/remove independently with ASCII case folding and first raw entry retention; migrate HTTP/GRPC request/response adapters to borrowed slices; cover supported append and invalid-input defensive ordering separately; correct all four guides so supported examples use one action and duplicate/cross-action fallback is clearly defensive pending D0090.
- Plan review: the initial documentation plan was rejected because it presented invalid duplicate/cross-action configuration as supported. The corrected plan states the one-action-per-name rule, tests fallback separately, proves append preserves existing multiple field lines, and contains all work in the eight frozen paths. Independent re-review approved it with no remaining Must Fix.
- Implementation: added one private shared immutable plan that consumes HTTP/GRPC configuration and retains the first ASCII-case-insensitive equivalent entry independently in each operation list. Both request and response adapters now borrow prepared slices with unchanged static errors and `set → add → remove` fallback ordering. Focused tests cover construction parity, ignored duplicates, invalid cross-action fallback, append-only multi-field preservation, ordered operations, errors, and zero hot-path preparation. Four EN/ZH guides now distinguish supported one-action configuration from defensive invalid-input behavior.
- Independent final review: LGTM with no Must Fix or Should Fix. The reviewer verified all HTTP/GRPC rule/backend and EdgionPlugins construction paths, real Pingora set/append/remove semantics, pending response operations, 1xx/101 and local-response boundaries, streaming, error cardinality, hot-path cost, four-guide parity, exact scope, and D0090/D0091/D0068 ownership.
- Verification: 39 Gateway API filter and 43 PluginRuntime tests passed. `cargo fmt --all`, `cargo check --locked --workspace --all-targets`, `cargo clippy --locked --workspace --all-targets`, `cargo build --locked -p edgion-gateway --bin edgion-gateway`, agent-doc, SSA force, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration-path guards passed; Clippy emitted only the existing warning set.
- Commit: pending.
