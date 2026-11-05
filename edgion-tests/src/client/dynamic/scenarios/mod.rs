// Scenario implementations. Each scenario lives in its own file.

pub mod acme_gatewayref_ownership;
mod ai_backend_cleanup;
pub mod ai_backend_lifecycle;
pub mod ai_backend_policy_integration;
pub mod ai_backend_secret_rotation;
pub mod ai_backend_weighted;
pub mod ai_guard_ordering;
pub mod all_endpoint_status_tls;
pub mod allowed_san_regex_ignore_case;
pub mod basic_auth_dynamic;
pub mod body_dynamic;
pub mod btlsp_ref_flap;
pub mod canary_hot_swap;
pub mod condition_refs_dynamic;
pub mod cors_validation;
pub mod credential_injector_dynamic;
pub mod csrf_secret_rotation;
pub mod delegation_parentref_gateway_collision;
pub mod direct_endpoint_port_resolution;
pub mod dns_resolution_flap;
pub mod dsl_dynamic;
pub mod dsl_terminate_dynamic;
pub mod dsl_xml_dynamic;
pub mod duplicate_host_policy_hot_reload;
pub mod edgion_config_data_ref_flap;
pub mod edgion_tls_last_known_good;
pub mod encrypted_active_health_checks;
pub mod form_json_transform;
pub mod gateway_class_params_flap;
pub mod gateway_dynamic;
pub mod grpc_route_delegation_dynamic;
pub mod header_cert_headers;
pub mod json_schema_validation;
pub mod l4_timeout_hot_reload;
pub mod link_sys_disabled_tls;
pub mod link_sys_provider_replacement;
pub mod observed_gen_high_freq;
pub mod rate_limit_local_dynamic;
pub mod rate_limit_local_overflow;
pub mod region_route_validation;
pub mod request_access_policy_hmac_dynamic;
pub mod request_mirror_reference_grant;
pub mod retained_listener_handles;
pub mod route_delegation_dynamic;
pub mod service_degrade_threshold;
pub mod sp_backend_change;
pub mod sp_weighted;
pub mod tcp_esp_reload;
pub mod tls_route_attachments;
pub mod tls_route_plugins_reload;
pub mod waf_dynamic;
pub mod wasm_runtime_ownership;

pub mod request_id_dynamic;

pub mod egc_runtime_hot_reload;
pub mod ext_proc_tls_policy_recovery;
pub mod http_route_duplicate_headers;
pub mod http_route_named_parameter_precedence;
pub mod http_route_path_prefix_trailing_slash;
pub mod plugin_policy;
pub mod preflight_policy_hot_reload;

pub mod ai_messages_binding;
pub mod ai_responses_continuation;
pub mod graphql_rate_limit;
pub mod grpc_json_transcoder;

pub mod graphql_operation_router;

pub mod graphql_guard;
pub mod graphql_safelist;
pub mod response_cache;

pub mod rest_to_graphql;

pub mod dsl_query_lookup;

pub mod metrics_lifetime;
pub mod shared_http_policy_admission;

pub mod jwt_claim_headers;
pub mod proxy_rewrite_host;

pub mod downstream_ecdh_curves;

pub mod auth_claim_headers;
pub mod oidc_session_expiry;
