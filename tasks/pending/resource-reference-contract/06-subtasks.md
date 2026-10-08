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

### R1 native Route implementation checkpoint

Independent Astra plan review approved the concrete actual-owner Route plan and
invalid-owner/denied-read amendments on baseline `8dab81bc6`. The initial scope was
13 production, 12 mirrored test and seven documentation paths; the coordinator
owns 07-progress.md separately. Only valid authored terminal loans mint Grant
queries. Invalid owners remain visible with rejected parent status and empty
derived execution state; denied targets retain dependencies without fresh
backend reads. Existing legal dependency cleanup and policy requeues remain.
Baseline and test identities: `/tmp/edgion-ref-native-before-8dab81bc`.
Implementation and focused verification are complete; independent final acceptance
and workspace gates remain with the coordinator. Gateway production, Grant-store,
worker, CRD and Cargo paths remain outside this slice.

R1 test-support amendment: independent Astra plan review approved the sole extra
`edgion-controller/tests/support/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
path (32 to 33 total). Its private fixture adapter preserves the full original rule
and explicit namespace while driving the actual owner-based AI resolver; no
production compatibility API is added. The first Controller helper compile failure
is retained in `/tmp/edgion-ref-native-helper-r1.log`; retry evidence follows below.

R1 fixture amendment: independent Astra review approved adding only the existing
`edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy_test.rs`
path (34 total): its wrong-group assertion now matches the canonical exact error.
Two existing empty-group Route fixtures explicitly provide the required empty
group, preserving their rejection assertions. The new effective-status test uses
Tokio for its processor queue. Handler run 1 (648 passed, 4 failed) is retained in
`/tmp/edgion-ref-native-handlers-r1.log`; no policy or ExtensionRef production code
changes were required.

R1 Gateway fixture amendment: independent Astra review approved the sole extra
`edgion-gateway/tests/unit/routes/http/proxy_http/pg_upstream_peer_test.rs` path
(35 total). The shared valid-Service fixture now supplies required port 80; the
existing named-jump denial test asserts both compiled targets are Services before
its unchanged denial checks. The prior absent port compiled both slots Invalid;
`/tmp/edgion-ref-native-gateway-jump-denial-r1.log` preserves that failure. Gateway
production and the separate R2 selector/peer scope remain unchanged.


R1 review handoff: the three independently approved test-only amendments bring the
allowlist to 35 paths (13 production, 15 test/support, seven documentation).
The baseline comparison preserves all 294 existing test identities and adds 16;
no original test was removed. Final scope, SHA-256 manifest, patch and identity
comparison are under `/tmp/edgion-ref-native-before-8dab81bc`.

Focused PASS results (log prefix `/tmp/edgion-ref-native-`): Resources refs 70
(`resources-r1`), Resources doctests 22 with 10 existing ignored
(`resources-docs-r1`), Controller owner helper 6 (`helper-r3`), handlers 652
(`handlers-r2`), resource processor 909 (`resource-processor-r1`), Gateway peer
98 (`gateway-peer-r1`, includes named-jump and direct denial regressions), and
Gateway selector 11 (`gateway-weight-r2`), gRPC routes 66 (`gateway-grpc-r1`),
and selected-Service context guards three (`gateway-context-r1`). The first helper compile failures
(`helper-r1` and `helper-r2`), handler fixture failures (`handlers-r1`), and the
old missing-port jump fixture failure remain in their original logs. Formatting
passed through `fmt-r8`; layout passed with 999 mounted files. The coordinator
owns final workspace check/Clippy/guards, fresh independent result review, and
the serial commit gate. This handoff does not close the whole reference task.


R1 final-review correction: the independent Astra review and workspace Clippy
identified one `manual_let_else` warning in the terminal Grant-query projection.
The equivalent `let ... else` retains the same private-constructor invariant and
changes no authority or API. Fresh `resources-r2` passed 70 tests and
`resources-docs-r2` passed 22 with 10 existing ignored; `fmt-r9` passed. Workspace
check r1 passed in 1m 48s; Clippy r1 exited zero in 1m 40s but contained that one
warning (also repeated for lib tests), so it is not a warning-free gate. The
coordinator owns check/Clippy r2 and final acceptance. The current root AGENTS was
re-read; no applicable nested AGENTS exists. No other source or test changed in
this correction. The final manifest, patch and identity comparison are refreshed.


### R2a owning HTTP and GRPC backend execution

Independent Astra plan review approved the concrete ten-part plan and explicit
factory, policy-capture, and error-mapping amendments: 51 paths (19 production,
23 test/support, nine documentation). Baseline originals and all 347 existing
test identities are preserved under `/tmp/edgion-ref-r2a-before-0fea0a618`.
The coordinator owns 07-progress.md, final workspace gates and commits.

Actual Route effective fields produce private cold Service targets with canonical
key, policy identity and checked port section. Selected handles, sticky candidates,
DirectEndpoint and final peer construction borrow these targets. Invalid owners
invoke no plugin factory; invalid or denied slots retain weights but invoke no
backend factory, policy/affinity lookup or backend dispatch. Context is a checked
diagnostic projection, never a source of identity. Captured absence, old request
Arcs, retry budgets, direct endpoint proofs and the ordinary AI 503 remain intact.

Malformed HTTP descriptors, including invalid required Service ports, fail with
BackendUnsupportedKind/500 before side effects; valid Service runtime section or
endpoint failures retain their existing 503 mapping. This deliberately corrects
the old invalid-port 503 path under the vendored weighted-invalid contract.
The historical `invalid_backend_with_service_key_still_captures` test keeps its
identity and adversarial inputs but changes the unsafe capture expectation to
None and zero actual policy reads. No L4, Service.type/DNS, filter implementation,
plugin outbound, schema, Controller, Common, Grant-store or worker changes.
Focused verification and fresh independent result review remain pending.

R2a test amendment: independent Astra plan review approved correcting the existing
DIU (seven helper-driven cases) and Canary (four) standard-Service fixture port
from absent to 80. These cases exercise valid named jumps, namespace/weight or
denial behavior; their identities and other inputs/assertions remain intact. Each
module adds an explicit absent-port, matching-name Invalid case proving zero jump
publication and the plugin's existing onInvalid Reject 403. The frozen 51 paths
are unchanged. The sole AI detached TLS helper caller now uses its existing
logical handle; the unused detached Service branch is removed, leaving one native
Service section lookup and the same permitted EdgionBackend whole-target lookup.

R2a implementation verification: Resources refs 71 passed; Resources doctests
22 passed plus 10 existing ignored. Final Gateway HTTP 482, backends 275, GRPC 68,
AI HTTP/dispatch/model 16/30/4, DirectEndpoint 11, DIU 10 and Canary 14 passed.
Gateway doctests passed 8 plus 7 existing ignored, including two new private
compiled-identity compile-fail cases. HTTP wire tests distinguish malformed
500/denied 500/ordinary AI 503 and preserve gRPC HTTP 200/status 14. Factory,
policy-read and sticky lookup counters cover rejected inputs with positive
controls; borrowing/allocation tests pass. Unit layout passed 1000 mounted files,
and agent-doc validation passed 1447 files. Final reviewer requested only
owning-source comment corrections after these behavior tests; no logic changed.

Logs use `/tmp/edgion-ref-r2a-*.log`: retained library/test compile failures r1/r2
were repaired before test-check-r3 passed; HTTP-r1 passed 481 before the final
wire case, and HTTP-r2 passed 482 on the completed behavior. Gateway-doc-r1 hit
ENOSPC; the coordinator removed only stale ws3 incremental caches and the r2
retry passed. The immutable baseline remains unchanged. Final scope has 49 changed
files within the approved 51, preserving all 347 original test identities and
adding 11. Explicit assertion/input amendments are the unsafe policy capture
expectation and the two named-jump fixture ports described above. Full workspace
gates and independent final result review remain coordinator-owned and pending;
this is not closure of later L4, Service.type/DNS, filter or outbound slices.


### R2b approved L4 implementation

Independent Astra plan review approved 56 paths (19 production, 21 tests, 16
documents) at d37b890a8. The immutable baseline and 185 source test identities
are under `/tmp/edgion-ref-r2b-before-d37b890a8`; the coordinator also captured
5110 compiled Gateway test identities. The coordinator owns 07, final gates
and commits. No source outside this scope is authorized.

Actual TCP/TLS/UDP owners compile first-rule effective loans into private Service
targets and original weighted slots. Selected loans gate discovery, preserving
denial priority and making malformed descriptors terminal. TLS valid live
endpoint/connect retries remain; the shared actual-unit one-attempt helper has
resolver positive/negative controls, while final review checks both real loops'
terminal returns. Existing stream route stages still precede backend selection.

Invalid owners create no executable unit or publication demand. Full publication
uses successful units; partial publication removes every touched key and adds
only final surviving units. Shared same-key add clears before initialization.
Raw manager/publication keys preserve an omitted namespace as a bare name; only
canonical backend identity defaults the namespace. Old selected Arcs and pinned
UDP sessions retain their existing lifetime. No policy/TLS capture, new retry
policy, Controller/schema changes or Service.type/DNS work is included.

Focused verification passed on the completed source: Gateway runtime 12
(overlaps backends), TCP 24, UDP 51, TLS 83, common helpers 37, shared delta 2,
stream bridge 6, backends 278 and selector 25. Gateway doctests passed 11 with
seven existing ignored, including three new private-unit compile-fail cases.
Logs use `/tmp/edgion-ref-r2b-*-r1.log`; fmt-r2 passed. The initial library
check passed with one unused legacy-predicate warning; the obsolete raw helper
was removed before final tests. No focused test failed and no existing fixture
input/assertion correction was needed beyond the approved actual-owner wrappers.

All 185 original scoped source test identities remain (195 now); the actual
Gateway binary listing retains all 5110 original identities (5120 now). Ignored
attributes are unchanged. The 55-file delta stays inside the 56-path allowlist:
19 production, 20 test and 16 documentation files changed. Final scope hashes,
reproducible digest algorithm, baseline checks, patch and identity comparison are
under `/tmp/edgion-ref-r2b-final`. No baseline files were changed.

Resolver counts test the actual shared closure boundary; the TLS tests exercise
the actual one-attempt helper and retry-delay decision. They are not connector
or wire-conformance tests; independent result review must inspect both real
loops' terminal returns. UDP lifecycle tests preserve existing session semantics.
Workspace gates, fresh independent Astra result acceptance and commit remain
coordinator-owned; R2c and later reference consumers are still excluded.

The coordinator's first diff and unit-layout guards found trailing task-document
blank lines and four test-only accessors in production source. The EOF whitespace
was removed; independently approved accessor moves put the unchanged methods in
their existing mirrored test modules. The failed guard logs remain under the
`/tmp/edgion-ref-r2b-guard-*-r1.log` names. Fresh fmt-r3 and layout-r2 passed;
runtime/TCP/UDP/TLS-r2 passed 12/24/51/83. Earlier final artifacts are archived as
`/tmp/edgion-ref-r2b-final-r1` and `-r2`; the refreshed final snapshot supersedes
their hashes. No test identities, assertions, inputs or production behavior changed.

### R2c approved native ExternalName implementation

Independent Astra plan review approved 31 paths at 5cf413bbc: three production,
three unit-test files (one new mirror), seven DNS fixtures and 18 documents.
The immutable baseline is `/tmp/edgion-ref-r2c-before-5cf413bbc`; all 15 original
source test identities, 5120 compiled Gateway identities and ignored attributes
must remain. The seven fixture files change only ten positive native backend
kind values from ServiceExternalName to Service. The coordinator owns 07 and SCM.

A private Service selection captures one AvailableServiceProjection, checked
section and membership-matched runtime for one peer attempt. Native HTTP/gRPC
strategy comes from that actual Service's type; its hostname, annotations, port
and appProtocol remain coherent through DNS awaits. Endpoint and sticky selection
reuse that capture; later attempts recapture geometry while retaining existing
request policy/TLS ownership. Resolver callbacks borrow caller-owned inputs at
the actual query boundary. Standard descriptors stay strict. Existing DNS modes,
cache identity, SSRF, TLS, live runtime eligibility and SP behavior remain; L4
self-resolution, annotation authority (R6), custom outbound modes (R7), schemas,
Controller processing and new stores are excluded. Fresh verification evidence
is recorded below; R2c acceptance completed as recorded in 07.

R2c amendment 01 received independent Astra plan PASS and coordinator execution
authorization: the effective allowlist is 32 paths. The existing
`edgion-gateway/tests/unit/backends/validation_test.rs` changes only its canonical
ServiceSectionKey import; all three additional original test identities and
bodies remain. Its immutable baseline is recorded separately under
`/tmp/edgion-ref-r2c-before-5cf413bbc/amendment-01`. The first focused compile
failed on that import and one new-test private-module import; a second attempt
found an Option/Result assertion mismatch in the new gRPC test. Both were corrected.
The original `/tmp` logs and compiled-identity listing were lost after a runtime
reset; earlier reported results are historical and their raw evidence is unavailable.
The baseline sources were recovered from immutable commit 5cf413bbc under
`tmp/resource-reference-contract/r2c-recovery-20261008`, reproducing the original
manifest SHA256 exactly. Fresh evidence uses
`tmp/resource-reference-contract/r2c-verification-20261008-r1`.

R2c fresh verification after recovery passed: native dispatch 12, complete
backends 290, peer 101, gRPC 68, DirectEndpoint-filtered 16, session-persistence
22, and Gateway doctests 11 with 7 existing ignored cases. Canonical prepare
completed successfully. The 18 original source test identities and ignored
attributes are unchanged; 12 new dispatch tests were added. Seven fixture files
contain exactly ten approved `ServiceExternalName` to `Service` kind-value
replacements; the import amendment changes no test body.

The direct canonical shared-topology run passed all four static DNS cases and
the complete DnsResolutionFlap scenario on one stack. Its unchanged scenario
awaits eight phases sequentially: failing, working, failing, working, bounded,
working, failing, working. The raw client reports scenario PASS and exit zero;
individual successful phases are intentionally silent, so their completion is
inferred from that unchanged fallible sequence, not a manufactured phase log.
The main runner's earlier macOS alias preflight did not pass; this is explicitly
a direct shared-topology result, not a full integration-runner or Kubernetes
conformance claim. Existing runner quality/capability, nofile, resource-sync,
single-Gateway and unknown-field gates passed. The sole owned stack was stopped;
the pre-existing token, current-run marker and both shared certificate trees
were restored exactly, with no matching product process left running. Original
credential backup copies were removed after restoration was verified; only
sanitized restoration metadata remains.

Fresh raw logs, binary/source hashes, ownership/restoration evidence and phase
source hashes are in `tmp/resource-reference-contract/r2c-verification-20261008-r1`.
The owned runtime logs remain under
`tmp/integration_testing/testing_20261008_084440`. Original historical `/tmp`
logs and the original compiled identity listing remain unavailable. The coordinator
reconstructed the full control from immutable HEAD: 5120 original Gateway tests
remain among 5132 current tests. Required workspace gates and both fresh independent
Astra result reviews passed; R2c committed locally as
`f65b863c7cda6ec9936569d91e7bd8897824c4b2`. See 07 for the accepted artifact hashes.

### R3a1 approved owning ExtensionRef Controller slice

Independent Astra plan review approved 23 existing paths at f65b863c7:
seven production, eight mirrored test and eight documentation files. The
immutable baseline is `tmp/resource-reference-contract/r3a1-baseline-20261008`
(manifest SHA256 `b87bb3ec17dab744857913a69095f4dc725dab138de16df950725a8cd02a78ea`).
All 188 original attributed test names and ignored cases must remain. The
coordinator owns 07, workspace gates, independent result reviews and commits.

Actual HTTP/GRPC owners mint borrowed filter loans. Authored loans ignore stale
namespace stamps; effective loans validate the existing resolved-rule stamp.
Original child identity and filter position are captured before cloning, while
the published root separately owns dependency wakeups. Valid child references
register before rule drops and before desired-generation RawStore reads. Parse
clears once for valid reconciliation; later root/status registration is additive.
An invalid-source standalone status call may withdraw stale dependencies and
derived publication state; this exception never applies to target/scope failures
under a valid owner. Invalid references register and read no plugin material. Existing filter scope/status/disabled semantics remain.
No new transport field, Grant capability, store, queue, handler I/O or mirror
behavior is introduced. Gateway early lookup/publication/request capture remains
open for the next serial R3a2 batch; this slice does not close ExtensionRef overall.
Focused verification is recorded below; independent final result acceptance is pending.

Fresh R3a1 focused verification passed on 2026-10-08: Resources reference tests
74; Resources doctests 24 passed with 10 existing ignored; Controller
`resource_processor` tests 918; readonly Gateway filter/publication/preflight/WAF
filters 53/23/23/65 respectively. These filtered sets may overlap. Exact commands,
raw output and exit codes remain under
`tmp/resource-reference-contract/r3a1-verification-20261008-r1` (Resources
`resources-refs-r3` / `resources-docs-r2`, Controller `controller-processor-r3`,
Gateway `gateway-{filters,publication,preflight,waf}-r1`). No full test suite or
live conformance run was performed. Original failed compilation logs and the
new lifecycle test's initial queue-order failure remain preserved; the latter
now proves the root wakeup without assuming ordering relative to child wakeups.
The initial implementation restored the existing test-support validator import
under `cfg(test)` in the approved HTTP handler; F1 below also uses that predicate
for production diagnostic discrimination. No support-file or allowlist expansion
was needed.

The coordinator's compiled identity control preserves all Resources 1922 and
Controller 1731 original test identities among 1925 and 1740 current tests;
the 188 scoped source identities remain among 200, with ignored cases unchanged.
Workspace gates, both fresh independent result reviews and the local commit
remain coordinator-owned pending gates. Gateway R3a2 and the rest of R3-R8 remain
open; these checks establish neither whole-ExtensionRef closure nor conformance.

R3a1-F1 independent result review found that HTTP name failures used the
unsupported-kind diagnostic even for exact `edgion.io/EdgionPlugins`. The narrow
correction plan received independent Astra scoped approval. The HTTP diagnostic
now discriminates with the existing pure group/kind predicate only after the
canonical validator returns `UnsupportedTarget`; no second name validator or
new error enum was introduced. Wrong group/kind wording and union precedence
remain unchanged. Added actual-owner cases cover empty/slash names at rule and
backend scope, valid-sibling partial status, all-invalid attachment rejection,
child-delegation drop reasons and absent plugin dependencies. Fresh correction
verification is recorded in `tmp/resource-reference-contract/r3a1-verification-20261008-r2`;
the frozen r2 correctness and subsequent lint results are recorded below.

F1 correction verification passed: `cargo test -p edgion-controller --lib
conf_mgr::sync_runtime::resource_processor::` completed with 921 passed,
0 failed and 822 filtered out (`controller-processor-r4.log`, exit 0, in the
r2 evidence directory). The three earlier correction attempts remain retained:
the new fixture lacked its Gateway, then supplied a borrowed runtime where an
Arc was required, then lacked the Tokio reactor required by the real workqueue.
The final fixture uses the existing async RawStore/Gateway processor pattern;
all requested assertions remain intact. Complete compiled identity comparison
preserves Resources 1922 among 1925 and Controller 1731 among 1743 tests; source
identities preserve 188 among 203, with unchanged ignore attributes. Both fresh
and deep Astra result reviewers returned SCOPED PASS for frozen r2 correctness.
Workspace checks and Clippy exited zero, but Clippy retained two introduced
`question_mark` warnings in new delegation test helpers; r2 was not warning-free.
The separately reviewed r3 cleanup uses the equivalent `?` early return in only
those helpers, preserving production behavior, assertions and test identities.
Fresh r3 Controller resource-processor verification passed 921 tests, with zero
failures/ignores and 822 filtered (`controller-processor-r1.log`, exit 0, under
`tmp/resource-reference-contract/r3a1-verification-20261008-r3`). Exact command,
log/binary hashes and compiled/source identity comparisons are retained there.
Resources 1922 -> 1925, Controller 1731 -> 1743 and source 188 -> 203 identities
remain unchanged from r2, with no original removals or changed ignore attributes.
Refreshed warning-free coordinator gates and renewed independent byte review
remain pending; R3a2 is still open.

### R3a2 Gateway owning ExtensionRef slice

R3a1 committed locally as `05e7debf53e87ab3709e7d2090c0b3e7356244ca`.
The reviewed R3a2 plan freezes 30 existing paths (10 production, 12 mirrored
tests, eight owning documents), with baseline manifest SHA256
`e8505004cc3d148af68e968c1f2db747e9a36bc7d8f9e501adac1f1c6fe94ff5`.
The baseline has 118 scoped source tests and 5132 compiled Gateway tests.
Actual Route/slot loans now supply optional cold keys for runtime construction,
publication/AI/adaptive discovery, captured request resolution, CORS/WAF and
access-log consumers. Invalid targets keep their positions and ExtensionRef
errors; preflight reuses their existing filter through the canonical dispatcher.
Malformed-union and policy gate precedence, selected-backend scope, and captured
hit/miss pinning remain intact. No Resources/Common/transport/Grant expansion,
legacy constructor shim, RequestMirror, descriptor or native ExternalAuth change.

The original gRPC and adaptive child-namespace fixtures now place the same stamped
rules in actual `resolved_rules`; expected locality and telemetry assertions stay
unchanged. The missing-payload CORS scanner fixture retains its explicit `None`
ordinal and original selected-config Arc assertion; separate OPTIONS tests prove
that malformed runtimes and invalid targets cannot admit preflight.

Preliminary moving-source runtime verification exited 101: 419 passed and the
existing dye producer inventory failed because its AiProxy fixture omitted `api`.
The raw failure remains preserved. Independent static comparison confirmed the
fixture was unchanged from committed HEAD; no executed baseline body is claimed.
A separately reviewed supporting amendment adds the existing dye test file as
path 31 and changes only its AiProxy config to `{"api":"openaiChat"}`, retaining
all four test identities and assertions. The final allowlist has 10 production,
13 test and eight documentation paths; the separate immutable dye baseline is
`tmp/resource-reference-contract/r3a2-baseline-20261008/dye-support-baseline.json`.
Stable r2 targeted selections passed before supplemental boundary tests were added.
Stable r3 then exposed a new CORS/WAF decoy fixture error: republishing compiled
plugin metadata had discarded resolved WAF bundles. The fixture now supplies the
original processed source through the existing publisher and asserts its WAF is
present before testing valid/invalid selection. No production change or weakened
expectation was used. The original r3 failure is retained.
All R3a2 raw commands, exits, logs and source/binary hashes are retained in
`tmp/resource-reference-contract/r3a2-verification-20261008-r1`. In-process
publication lifecycle evidence does not establish live Kubernetes admission,
whole ExtensionRef/conformance acceptance, or completion of R3b/R3c and R4-R8.

Final stable r4 verification on 2026-10-08 passed: full Gateway plugin runtime
424; exact dye producer inventory one (included in runtime); publication 25;
adaptive routing 21; request filter 29; positional resolver nine; HTTP match units
12; gRPC match units nine; access-log extensions 15; context pinning eight; native
WAF 38; early request capture 11; plugin store 41. Selections can overlap and are
not a full-suite result. All 23 Rust file hashes remained unchanged throughout
this compile and test run. Source identities are 122 to 134, including the four
approved dye tests; complete compiled identities are 5132 to 5144. Exactly 12
new tests were added, with no old identity removed and scoped ignore attributes
unchanged. Source/binary/log hashes and comparisons are retained alongside the
raw commands. Scoped formatting and `git diff --check` passed. No executable
documentation example changed, so no Gateway doctest run was required.

Coordinator workspace checks, guards, immutable final reviews and commit remain
pending. These in-process tests do not claim live Kubernetes admission or whole
resource-reference-contract/conformance completion.

R3a2-F1: the first independent r4 result review blocked on one new test-only
`clippy::disallowed_types` warning: the real-context cache fixture used
`std::sync::Mutex`. Workspace check and all 11 repository guards passed; Clippy
exited zero but was not warning-free. A separately reviewed precision correction
changes only that fixture to `parking_lot::Mutex` and its two infallible `.lock()`
calls, preserving all assertions and synchronous guard lifetimes. Stable r5
verification passed the complete runtime selection (424) and the exact cache
regression (one, also included in runtime). Source identities remain 134 (122
baseline plus 12 additions); compiled identities remain 5144 (5132 baseline plus
12 additions), with no removals or changed scoped ignore attributes. All 23 Rust
hashes stayed fixed during r5 compilation/tests. Other unchanged selections retain
their explicitly recorded r4 evidence. Raw r4 lint, earlier failures, and new r5
commands/exits/source/binary hashes are preserved in the same verification
directory. Refreshed workspace lint/guards, replacement independent result
reviews and commit remain coordinator-owned pending gates; no warning-free final
lint or overall conformance acceptance is claimed here.

## R3b current execution boundary

The independently approved R3b scope is 33 existing files after the narrow
supporting-test and tracing-registry amendments: 12 production, 14 mirrored tests,
six owning docs and one Python guard.
Root owns progress07 and commit gates. The exact plan, amendment and immutable
baseline are in `tmp/resource-reference-contract/r3b-plan-20261008` and
`r3b-baseline-20261008`.

Actual HTTP/EP mirror owner loans, authored-only Grant queries, dependencies before
permission reads, additive child-edge retention and cold Gateway targets are
implemented. The HTTP raw-None wake-key correction changes two expected owners in
`handlers/route_backend_refs_test.rs`, preserving all six IDs, all target/default,
replacement/cleanup/recovery assertions and all non-HTTP expectations.

Stable r4 verification passed Controller 927 tests (including all six supporting
tests), Resources 1601 tests plus 27 doctests/10 ignored, and Gateway 2254 plugin
tests/17 ignored. Scoped source identities are 297 to 311 with no removals or
changed ignores; complete compiled lists are 1925 to 1929, 1743 to 1749 and
5144 to 5148, with no removals. All earlier compile/assertion failures remain
recorded. See [04-implementation.md](04-implementation.md) for evidence detail and
[07-progress.md](07-progress.md) for workspace checks, independent result reviews
and commit authority, which remain separate gates.

Other native omitted-namespace Service worker keys were an explicit R4 follow-up,
not part of R3b acceptance; the bounded correction is recorded in R4a below. Mirror in-process tests do not establish live
conformance; descriptor R3c and remaining finite-contract work remain open.

The first actual R3b review returned SCOPED BLOCK for R3b-F1: a test-only
ResourceMeta import was unconditional in production. Root check/Clippy exited zero
with that warning, so the clean gate failed. The correction gates precisely that
import with `#[cfg(test)]`; fresh Gateway r5 verification passed 2254 tests
with 17 ignored, while unchanged Resources/Controller r4 evidence is retained.
Complete compiled and scoped source identities remain unchanged from the r4
comparison, with no removed original test or changed ignore. The exact cold-helper
tracing registration amendment passed its guard and 18 unchanged Python tests.
Corrected workspace gates and both independent actual-result approvals are still
required; no final acceptance is inferred from these corrections.

## R3c handoff (2026-10-08)

R3b completed at local commit `16e7be6dce9d9d7ffc9dd70fa6889a557043aace`;
earlier pending wording records its historical handoff. R3c implements the approved
15-path descriptor boundary: canonical name checks, actual request-entry loans
before material validation/cache/construction, and shared material-free unsupported
ConfigMap stage validation. Inline diagnostics and body/transport contracts remain.

Verification: Resources r2 1602 resource tests plus 13 transcoding tests; unchanged
Resources r1 doctests 27 passed/10 ignored; Controller r4 929 passed; Gateway r3
2260 passed/17 ignored. Raw r2's new dye fixture spelling failure is retained.
Final evidence, per-run source/binary hashes and identity controls are under
`tmp/resource-reference-contract/r3c-verification-20261008-r1`. All 235 old scoped
IDs/assertions remain, with 10 additions and no changed ignores. Compiled lists:
Resources 1929→1931, Controller 1749→1751, Gateway 5148→5154, no removals.

Worker coverage uses actual queues/raw stores/process_work_item through ConfigMap,
EP, Route and Gateway. Namespaced target/current UID and dependency/status assertions
are explicit; EP Accepted does not control Route Accepted. Raw-None EP recovery
and notification do not close R4 semantic-target/raw-key lookup or generic status
reads. Gateway cache/decoy/policy-generation tests are in-process evidence only.
The initial candidate's first Astra PASS and 14 clean gates (check 47.479 seconds,
Clippy 80.239 seconds) are historical: the second review returned R3c-F1 BLOCK for
upstream eager descriptor copies. That initial PASS is superseded. Corrected-candidate
acceptance, renewed reviews and commit status are recorded in coordinator-owned
[progress07](07-progress.md). No push or whole-contract/conformance acceptance is claimed.

R3c-F1 correction: the independently approved scope is now 19 existing paths,
including four supporting originals and 253 baseline test IDs. A shared actual-owner
cold-copy helper covers publication, generation policy views and direct compilation;
rate-dependency alias checks borrow entries. Raw input retention/equality remains
opaque bookkeeping, and masked-view copy avoidance does not describe earlier
eligible source copies. All fields/ordinals and R4/live limitations are preserved.

Fresh F1 r1 passed Resources 1602+13 tests, doctests 27/10 ignored, Controller 929,
and Gateway 2264/17 ignored. Deterministic copy-callback, field-preservation and
publication/cache/mask/recreation/old-Arc controls passed. Source IDs 253→267 and
compiled IDs Resources 1929→1931, Controller 1749→1751, Gateway 5148→5158 show
no removals; all original assertions and previous R3c tests remain. Evidence is in
`tmp/resource-reference-contract/r3c-f1-verification-20261008-r1`. Renewed workspace
gates and two actual-result reviews are required before coordinator acceptance;
the earlier first PASS does not accept the correction.


### R4a execution evidence

The 14-path batch makes GRPC/TCP/TLS/UDP Service dependency owners preserve their
actual raw namespace for register/replace/delete and policy notifications. Target
and Gateway index defaults remain semantic `default`. The shared helpers retain
forwarding-only policy selection and dropped-target notifications.

Stable focused Controller verification passed 931 tests; source IDs 13 to 15 and
compiled IDs 1751 to 1753 preserve all originals and ignores. Actual worker tests
cover both namespace forms, Service/Slice events, latest UID/generation,
ordinary same-UID Service updates with changed resource version and port mapping,
coalescing, replacement, deletion/recreation and old/current policy wakes. Two
NEW-fixture failures and their corrections are retained in
`tmp/resource-reference-contract/r4a-verification-20261008-r1`.
See [04-implementation.md](04-implementation.md) for scope and evidence, and
[07-progress.md](07-progress.md) for final gates, independent reviews and commit.
Other event-source identities, Admin/source collision, raw-None EP lookup,
generic EP status scans and live conformance remain separate work.


### R4b execution evidence

The ten-path readonly-status batch removes registry-string material authority
and restores CredentialInjector's missing/denied diagnostics at its actual field
classifier. Other existing bound families, exact aggregate deduplication,
activity/stage rules and persist-off LinkSys policies remain intact.

Stable focused verification passed 935 Controller tests. Source IDs 136 to 140
and compiled IDs 1753 to 1757 retain all originals and ignores. Canonical-call
observers, stale-edge/fresh-status tests and actual Secret/ConfigMap workers
cover ordinary same-UID updates as well as create/delete/recreate. Internal
Grant-store read counts and live delivery are not claimed. Raw failures and
hash/identity controls remain in `tmp/resource-reference-contract/r4b-verification-20261008-r1`.
See [04-implementation.md](04-implementation.md) for bounded evidence and
[07-progress.md](07-progress.md) for final gates, independent reviews and commit.
Admin/source collision and raw-None EP semantic lookup remain separate R4 work.
