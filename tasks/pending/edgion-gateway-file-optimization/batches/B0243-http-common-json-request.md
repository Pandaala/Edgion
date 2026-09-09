# B0243 — HTTP Common JSON Request

## Scope

- Pipeline state: changed; implementation, independent review and verification complete, awaiting local commit.
- Active anchor: `edgion-gateway`.
- Baseline: `cf359498`.
- Target: `edgion-gateway/src/plugins/http/common/json_request.rs`.
- Inspect complete shared JSON request construction, byte bounds, error semantics, all callers and tests.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.

## Analysis Outcome

- Decision: change, moderate. Target blob `849487733f58da9545efe80e003ea9c4433339d6`; JsonSchemaValidation, ProxyRewrite and FormJsonTransform caller blobs are unchanged at `cf359498`.
- All three request-path consumers immediately allocate and clone `Content-Type` and `Content-Encoding` values only to classify them before body access. The canonical `request_header_map()` exposes the same duplicate/raw values by reference and no borrow must survive an await or mutation.
- Frozen scope: target plus the three caller plugin files. Both classifiers should accept a borrowed `HeaderMap`, use `get_all` directly, preserve exact JSON/form media-type, charset, duplicate, invalid-byte and identity-encoding semantics, and end the borrow before body work.
- Inline target tests should use real HeaderMaps and add invalid-byte Content-Type/Content-Encoding regressions. Existing consumer tests remain the verification floor. No integration, BodyStorage, failure, logging, schema or public protocol change is needed.

## Outcome

- Changed both shared classifiers to inspect the borrowed request `HeaderMap` directly, removing six temporary header-vector/string clone paths across JsonSchemaValidation, ProxyRewrite and FormJsonTransform.
- Preserved fail-close handling for missing, duplicate and non-UTF-8 Content-Type values; JSON/form MIME and charset rules; and absent-or-single-identity Content-Encoding. The borrow ends synchronously before body access or request mutation.
- Converted the unit fixtures to real HeaderMaps and added duplicate-charset plus raw non-UTF-8 Content-Type/Content-Encoding regressions. No integration, schema, logging, metric or public protocol change was made.
- Independent final reviewer `b0243_final_review`: LGTM with no findings; confirmed exact four-file technical scope, caller lifetimes and preserved header semantics.
- Verification: shared classifier 5/5, JsonSchemaValidation 9/9, ProxyRewrite 12/12 and FormJsonTransform 8/8. Workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed.
- Commit: pending; never push.
