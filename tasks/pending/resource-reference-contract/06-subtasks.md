# Serial subtasks and frozen candidates

One serial mutation lane, coordinated by the primary agent and implemented by
delegated IMPL subagents under the latest user instruction. This is a cross-cutting
feature refactor, not a systematic quality campaign. Candidate files may change
only for finite manifest-related behavior. Inventory is not proof of coverage.

| Unit | Completion boundary | Status |
| --- | --- | --- |
| 1 | Pure/wire/source-schema matrix and owning adapters | implementation in progress; plan review passed |
| 2 | Bound material lookup, exact Grant/revision agreement and lifecycle | pending |
| 3 | Canonical supported routes/filter/policy targets through final dial | pending |
| 4 | Fixed refs/selectors/output identities; full lookup-site audit | pending |
| Final | Required checks, repeated independent final review, docs/closure | pending |

This original four-unit table describes broad scope rather than current completion.
Use [07-progress.md](07-progress.md) for serial execution and commit status. A
provider's scoped PASS does not close its enclosing unit or the overall task.

## Frozen existing Rust candidate files

~~~text
edgion-common/src/link_sys_core/http_client.rs
edgion-common/src/link_sys_core/outbound_tls.rs
edgion-common/src/link_sys_core/providers/etcd/config_mapping.rs
edgion-common/src/link_sys_core/tls.rs
edgion-controller/src/conf_mgr/conf_center/etcd/resource_controller.rs
edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs
edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs
edgion-controller/src/conf_mgr/runtime.rs
edgion-controller/src/conf_mgr/schema_validator.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/ca_refs.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/mod.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/pipeline.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/request_secrets.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential_source_state.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/edgion_config_data_utils/mod.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/edgion_config_data_utils/ref_manager.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/gateway_route_index.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/auth_spec.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/config_data_ref_mark.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/dns_resolver_guard.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_acme.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/condition_tree.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/request_access_policy_data.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/ai_guard.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/basic_auth.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/credential_injector.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/csrf.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/ext_proc.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/forward_auth.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/guardrail.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/header_cert_auth.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwe.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwt.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/key_auth.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/ldap_auth.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/mod.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/openid_connect.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/wasm.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/request_mirror.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/udp_route.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_grant/edgion_config_data_grant.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_grant/store.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_grant/validator.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_diff.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_ownership.rs
edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs
edgion-controller/src/config/conf_center/etcd.rs
edgion-controller/src/services/acme/secret_io.rs
edgion-controller/src/services/acme/service.rs
edgion-controller/src/services/credential_manager/secret_io.rs
edgion-controller/src/services/credential_manager/service.rs
edgion-controller/src/services/dns_resolver/service.rs
edgion-controller/src/services/dns_resolver/slice_builder.rs
edgion-gateway/src/ai/guards/request/evaluator.rs
edgion-gateway/src/backends/health/check/encrypted.rs
edgion-gateway/src/backends/mod.rs
edgion-gateway/src/backends/service_kind.rs
edgion-gateway/src/ctx.rs
edgion-gateway/src/link_sys/providers/elasticsearch/log_sender.rs
edgion-gateway/src/link_sys/providers/kafka/client.rs
edgion-gateway/src/link_sys/providers/kafka/log_sender.rs
edgion-gateway/src/link_sys/providers/otlp/client.rs
edgion-gateway/src/link_sys/providers/otlp/log_sender.rs
edgion-gateway/src/link_sys/providers/otlp/transport.rs
edgion-gateway/src/link_sys/providers/redis/config_mapping.rs
edgion-gateway/src/plugins/http/ai_guard/mod.rs
edgion-gateway/src/plugins/http/all_endpoint_status/transport.rs
edgion-gateway/src/plugins/http/canary/plugin.rs
edgion-gateway/src/plugins/http/common/outbound/client.rs
edgion-gateway/src/plugins/http/common/outbound/target.rs
edgion-gateway/src/plugins/http/ext_proc/client.rs
edgion-gateway/src/plugins/http/forward_auth/plugin.rs
edgion-gateway/src/plugins/http/guardrail/mod.rs
edgion-gateway/src/plugins/http/ldap_auth/plugin.rs
edgion-gateway/src/plugins/http/openid_connect/plugin.rs
edgion-gateway/src/plugins/http/openid_connect/session_cookie.rs
edgion-gateway/src/plugins/publication/inputs.rs
edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs
edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs
edgion-gateway/src/routes/common.rs
edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs
edgion-gateway/src/routes/tcp/tcp_route_table.rs
edgion-gateway/src/routes/tls/gateway_tls_routes.rs
edgion-gateway/src/routes/udp/udp_route_table.rs
edgion-gateway/src/runtime/matching/tls.rs
edgion-resources/src/constants/annotations.rs
edgion-resources/src/constants/labels.rs
edgion-resources/src/lib.rs
edgion-resources/src/matcher/ip_cidr_helpers.rs
edgion-resources/src/output.rs
edgion-resources/src/resources/backend_tls_policy.rs
edgion-resources/src/resources/common/external_http.rs
edgion-resources/src/resources/common/kubernetes_name.rs
edgion-resources/src/resources/common/mod.rs
edgion-resources/src/resources/common/outbound_tls.rs
edgion-resources/src/resources/common/parent_identity.rs
edgion-resources/src/resources/common/request_template.rs
edgion-resources/src/resources/common/secret_object_ref.rs
edgion-resources/src/resources/common/secret_slot.rs
edgion-resources/src/resources/common/service_backend_kind.rs
edgion-resources/src/resources/config_data_ref.rs
edgion-resources/src/resources/edgion_acme.rs
edgion-resources/src/resources/edgion_backend.rs
edgion-resources/src/resources/edgion_backend_traffic_policy.rs
edgion-resources/src/resources/edgion_config_data/mod.rs
edgion-resources/src/resources/edgion_config_data/waf_policy.rs
edgion-resources/src/resources/edgion_gateway_config.rs
edgion-resources/src/resources/edgion_plugins/conditions.rs
edgion-resources/src/resources/edgion_plugins/hmac_match.rs
edgion-resources/src/resources/edgion_plugins/mod.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_guard.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/basic_auth.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/canary.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/credential_injector.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/ctx_set.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/forward_auth.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/guardrail.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/header_cert_auth.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/hmac_auth.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/ip_restriction.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/jwe_decrypt.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/jwt_auth.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/key_auth.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/linksys_ref.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/mod.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/region_route.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/request_access_policy.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs
edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm_oci.rs
edgion-resources/src/resources/edgion_stream_plugins/connection_ip_restriction_rules.rs
edgion-resources/src/resources/edgion_stream_plugins/global_connection_ip_restriction.rs
edgion-resources/src/resources/edgion_tls.rs
edgion-resources/src/resources/extension_ref.rs
edgion-resources/src/resources/gateway.rs
edgion-resources/src/resources/gateway_class.rs
edgion-resources/src/resources/grpc_route.rs
edgion-resources/src/resources/health_check.rs
edgion-resources/src/resources/http_route.rs
edgion-resources/src/resources/http_route_ai_admission.rs
edgion-resources/src/resources/http_route_preparse.rs
edgion-resources/src/resources/link_sys/common.rs
edgion-resources/src/resources/link_sys/credential_source.rs
edgion-resources/src/resources/link_sys/mod.rs
edgion-resources/src/resources/link_sys/otlp.rs
edgion-resources/src/resources/mod.rs
edgion-resources/src/resources/proxy_protocol.rs
edgion-resources/src/resources/reference_grant.rs
edgion-resources/src/resources/tcp_route.rs
edgion-resources/src/resources/tls_route.rs
edgion-resources/src/resources/udp_route.rs
~~~

## Supporting additions

- New edgion-resources/src/resources/refs/{mod,identity,contracts,targets,grants,fields,validation}.rs, split only as needed.
- New tests/unit/resources/refs mirror plus existing mirrored tests for affected modules.
- Scoped generator/drift tests under cicd/checks and generated reference JSON under edgion-resources/src/contracts, reusing existing transform infrastructure.
- Existing Admin schema-validator callers: record exact files before edits.
- Affected custom CRD reference interiors only; standard vendored CRDs frozen.
- Existing shared integration reference suites/fixtures: record exact case paths before edits; no second topology or blanket rewrites.
- Owning resource/reference architecture, user default/alias guidance and original knowledge only; narrow repair of known ai-cost-weighted-request-output-bound-parsing.md link.
- Python Gateway producer parity fixtures only if representation changes; retain current semantics.

## Consumer audit

Before each unit, record concrete changed paths, finite manifest rows, owning
adapter, actual lookup/dial and tests. An unlisted supporting file needs recorded
call-chain evidence; material deviations require independent plan re-review.

## Unit 1 supporting import migration

The moved canonical ExtensionRef owner requires import-only edits in:

- edgion-gateway/src/ai/routing/adaptive.rs
- edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs
- edgion-gateway/src/plugins/runtime/plugin_runtime.rs
- edgion-gateway/src/plugins/publication/publisher.rs

No routing behavior beyond the shared reference contract is changed there.

## Unit 1 CA wire split supporting edits

Exact custom CA construction/type users identified by symbol search:

- edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/credential/ca_refs_test.rs
- edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/link_sys_test.rs
- edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins_test.rs
- edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config_test.rs
- edgion-controller/src/config/conf_center/etcd.rs
- edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/ca_refs.rs
- edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs
- edgion-common/tests/unit/link_sys_core/outbound_tls_test.rs
- edgion-common/tests/unit/link_sys_core/credential_test.rs
- edgion-common/tests/unit/link_sys_core/tls_test.rs
- edgion-gateway/tests/otlp_transport.rs
- edgion-gateway/tests/unit/link_sys/providers/redis/config_mapping_test.rs
- edgion-gateway/tests/unit/link_sys/providers/kafka/client_test/tls_config_tests.rs
- edgion-gateway/tests/unit/config/edgion_gateway/conf_handler_impl_test.rs
- edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test.rs
- edgion-gateway/tests/unit/plugins/http/ldap_auth/plugin_test.rs
- edgion-gateway/tests/unit/plugins/http/ext_proc/client_test.rs

CA split compiler evidence adds edgion-common/src/link_sys_core/fingerprint.rs: its consumed TLS policy hash reads the changed custom CA group field. Preserve omission as the effective empty group; final owning-adapter migration will supply canonical identities.

## Unit 1 optional-group constructor migration

Exact constructor users changed for the three presence-aware wire fields:

- edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/ref_grant/validator_test.rs
- edgion-gateway/tests/unit/plugins/http/request_mirror/plugin_test/arming_tests.rs
- edgion-gateway/tests/unit/plugins/http/common/outbound/client_test.rs
- edgion-gateway/tests/unit/backends/health/check/manager_test.rs
- edgion-gateway/tests/unit/backends/policy/backend_traffic/store_test.rs
- edgion-gateway/tests/unit/backends/policy/backend_traffic/conf_handler_impl_test.rs
- edgion-gateway/tests/unit/routes/http/proxy_http/pg_upstream_peer_test.rs
- edgion-resources/tests/unit/resources/edgion_backend_traffic_policy_test.rs
- edgion-resources/src/resources/http_route.rs
- edgion-resources/src/resources/edgion_backend_traffic_policy.rs
- edgion-resources/src/resources/common/external_http.rs

Optional mirror group consumers additionally require edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_delegation.rs: its child-local mirror grant query must preserve omission rather than nesting Option.

## Backend compilation support

The final Service dial guard and cold compiled target classification require
`edgion-gateway/src/routes/http/match_unit.rs` and
`edgion-gateway/src/routes/grpc/match_unit.rs`, plus their mirrored unit tests.
These supporting edits preserve weighted invalid slots and pin validated targets.

## Source validation support

Reuse `edgion-resources/src/crd_contract.rs` to return its temporary normalized
projection, and embed the generated finite reference contract under contracts/.
The existing `edgion-controller/src/conf_mgr/conf_center/common.rs` alpha source
checks already call SchemaValidator before promotion; preserve that boundary.
Do not add rejection to canonical/beta watcher ingestion. SchemaValidator tests
cover the existing source seam. No Kubernetes watcher full-schema
validation or vendored CRD edit is authorized by these support paths.

Direct endpoint authorization also compiles the actual validated HTTP Service
identity in `edgion-gateway/src/backends/validation.rs`; its mirrored unit tests
must prove invalid identities cannot mint a direct endpoint permission.

The new reference drift check is wired beside the existing transform guard in
`.github/workflows/ci.yml`; its Python regression test covers the exact source
inventory and fails on removal of a selected reference field.

Async owning-reference verification also uses `edgion-controller/src/lib.rs`
only to attach the test-only module at
`edgion-controller/tests/unit/lib_test/reference_test_support.rs`. The shared fixture
constructs real typed owners and installs existing LocalRawStore-backed
processors; it adds no production visibility or permissive owner fallback.
