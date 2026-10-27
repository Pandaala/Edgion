# Request Cookie implementation manifest

Baseline: `6069963764e2435aaa834aaa8aa56028b0dec67a`.
Approved by independent core and transport reviewers before production edits.
One production writer; root coordinates verification and documentation.

## Production and benchmark files

All paths below are relative to `edgion-gateway/` unless otherwise stated.

- `src/request_cookie.rs` (new canonical pure module)
- `src/lib.rs`
- `Cargo.toml`
- `tests/benches/request_cookie.rs`
- `src/plugins/runtime/traits/session.rs`
- `src/plugins/runtime/traits/header_ext.rs`
- `src/plugins/runtime/session_adapter.rs`
- `src/plugins/http/common/auth_common.rs`
- `src/plugins/http/key_auth/plugin.rs`
- `src/plugins/http/jwt_auth/plugin.rs`
- `src/plugins/http/jwt_auth/request_stage.rs`
- `src/plugins/http/openid_connect/session_cookie.rs`
- `src/plugins/http/openid_connect/request_filter_impl.rs`
- `src/plugins/http/openid_connect/callback.rs` (necessary helper signature only)
- `src/plugins/http/csrf/plugin.rs`
- `src/plugins/http/dsl/lang/builtins.rs`
- `src/plugins/http/common/outbound/request.rs`
- `src/link_sys/template/engine.rs`
- `src/routes/http/session_persistence.rs`
- `src/observe/access_log/log_ext.rs`
- `src/backends/selector.rs` (preserve first-field Cookie scope)
- Repository-relative `cicd/checks/check_request_cookie_ownership.py` (new)

The conditional cache is benchmark-only until adoption gates pass. Context, AI
restore and cache invalidation edits require a separate measured/reviewed amendment.
No WAF leaf grammar, response Set-Cookie, Query behavior, schema or dependencies.

## Unit-test files

Relative to `edgion-gateway/`:

- `tests/unit/request_cookie_test.rs`
- `tests/unit/plugins/runtime/session_adapter_test.rs`
- `tests/unit/plugins/runtime/session_adapter_test/record_outcome_tests.rs`
- `tests/unit/plugins/http/common/auth_common_test.rs`
- `tests/unit/plugins/http/common/outbound/request_test.rs`
- `tests/unit/link_sys/template/engine_test.rs`
- `tests/unit/link_sys/providers/webhook/runtime_test.rs`
- `tests/unit/plugins/http/key_auth/plugin_test.rs`
- `tests/unit/plugins/http/jwt_auth/plugin_test.rs`
- `tests/unit/plugins/http/jwt_auth/mod_test/token_source.rs`
- `tests/unit/plugins/http/openid_connect/plugin_test.rs`
- `tests/unit/plugins/http/csrf/plugin_test.rs`
- `tests/unit/plugins/http/ext_proc/mutation_test.rs`
- `tests/unit/backends/selector_test.rs`
- `tests/unit/routes/http/session_persistence_test.rs`
- `tests/unit/observe/access_log/log_ext_test.rs`

Repository-relative `edgion-tests/tests/dsl_redis_rate_limit.rs` may add required
manual PluginSession fixture methods only. Integration fixture files must be
identified and reviewed before editing; do not freeze an arbitrary whole subtree.
Compiler-required test expectation migrations need an explicit small amendment.

## Records and closure

Root owns this manifest, the existing request-cookie-management plan and new
verification record. Update existing owning Cookie knowledge pages after verifying
behavior; add a small architecture/API note only if the existing plugin guide is
insufficient. Close `task/review-data-panel/auth-01-strip-survivor-storage.md` only
after verification and independent final review; preserve shared historical indexes.
No unrelated task cleanup or mutation of `impl/`.

## Reviewed live-chain amendment

Independent transport and core reviewers approved these exact repository paths:

- `edgion-tests/integration/conf/EdgionPlugins/WebhookKeyGet/20-cookie-cleanup-chain.yaml`
- `edgion-tests/src/client/suites/edgion_plugins/webhook_key_get/webhook_key_get.rs`

Use existing mirror capture/query endpoints for three webhook projections, mirror
and main backend. No test-server edit. A real adapter get_all(COOKIE) regression
complements capture-store assertions because its JSON header map collapses repeated
field names. Failure observations cover a complete bounded window and verify the
invalid-byte request actually reaches Gateway and returns 400. Unique trace IDs;
never reset the shared capture store.

## Reviewed failure and shared-suite amendments

Both independent reviewers approved runtime-chain tests and K8s portability:

- `edgion-gateway/tests/unit/plugins/runtime/plugin_runtime_test.rs`
- `edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test/dependency_error_tests.rs`
- `edgion-tests/k8s/conf/EdgionPlugins/WebhookKeyGet/20-cookie-cleanup-chain.yaml`
- `edgion-tests/k8s/scripts/common/run_test_client.sh`
- `edgion-tests/k8s/scripts/common/run_test_client_batch.sh`

The shared suite aggregates captures over deduplicated backend Pod inventory;
missing inventory and individual query errors fail explicitly. All new K8s
resources use edgion-test and the existing test-server selector. No global reset.

Documentation changes are limited to existing KeyAuth/JwtAuth stripping guards,
OIDC session-cookie ownership, JwtAuth query-token borrowed extraction, the plugin
development guide, and English/Chinese KeyAuth, JwtAuth and OpenID Connect guides.

## Related fixture correction

Independent transport review confirms the existing ExtProc explicit-empty Query
fixture must expect `/api?`, matching baseline `compose_uri`. Only its expectation
and comment change; no production Query behavior is changed.

A second independent review approved the exact supporting test path
`edgion-gateway/tests/unit/ai/connectors/provider_test.rs`: initialize its synthetic
request's Query owner before calling the adapter, as the real request entry does.
The baseline fixture omitted this step. All provider assertions and production
behavior remain unchanged.
