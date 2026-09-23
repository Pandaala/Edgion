// Scenario registry. To add a new scenario, append a single entry to this list.

use crate::dynamic::scenario::Scenario;

/// All registered dynamic scenarios.
pub fn all() -> Vec<&'static Scenario> {
    vec![
        crate::dynamic::scenarios::duplicate_host_policy_hot_reload::SCENARIO,
        crate::dynamic::scenarios::preflight_policy_hot_reload::SCENARIO,
        crate::dynamic::scenarios::egc_runtime_hot_reload::SCENARIO,
        crate::dynamic::scenarios::cors_validation::SCENARIO,
        crate::dynamic::scenarios::acme_gatewayref_ownership::SCENARIO,
        crate::dynamic::scenarios::http_route_named_parameter_precedence::SCENARIO,
        crate::dynamic::scenarios::http_route_path_prefix_trailing_slash::SCENARIO,
        crate::dynamic::scenarios::http_route_duplicate_headers::SCENARIO,
        crate::dynamic::scenarios::ext_proc_tls_policy_recovery::SCENARIO,
        crate::dynamic::scenarios::plugin_policy::SCENARIO,
        crate::dynamic::scenarios::delegation_parentref_gateway_collision::HTTP_SCENARIO,
        crate::dynamic::scenarios::delegation_parentref_gateway_collision::GRPC_SCENARIO,
        crate::dynamic::scenarios::request_mirror_reference_grant::SCENARIO,
        crate::dynamic::scenarios::service_degrade_threshold::SCENARIO,
        crate::dynamic::scenarios::link_sys_disabled_tls::SCENARIO,
        crate::dynamic::scenarios::link_sys_provider_replacement::SCENARIO,
        crate::dynamic::scenarios::direct_endpoint_port_resolution::SCENARIO,
        crate::dynamic::scenarios::all_endpoint_status_tls::SCENARIO,
        crate::dynamic::scenarios::encrypted_active_health_checks::SCENARIO,
        crate::dynamic::scenarios::ai_backend_lifecycle::SCENARIO,
        crate::dynamic::scenarios::ai_backend_policy_integration::SCENARIO,
        crate::dynamic::scenarios::ai_backend_secret_rotation::SCENARIO,
        crate::dynamic::scenarios::ai_backend_weighted::SCENARIO,
        crate::dynamic::scenarios::ai_guard_ordering::SCENARIO,
        crate::dynamic::scenarios::gateway_dynamic::SCENARIO,
        crate::dynamic::scenarios::dsl_dynamic::SCENARIO,
        crate::dynamic::scenarios::dsl_terminate_dynamic::SCENARIO,
        crate::dynamic::scenarios::body_dynamic::SCENARIO,
        crate::dynamic::scenarios::condition_refs_dynamic::SCENARIO,
        crate::dynamic::scenarios::credential_injector_dynamic::SCENARIO,
        crate::dynamic::scenarios::csrf_secret_rotation::SCENARIO,
        crate::dynamic::scenarios::json_schema_validation::SCENARIO,
        crate::dynamic::scenarios::form_json_transform::SCENARIO,
        crate::dynamic::scenarios::basic_auth_dynamic::SCENARIO,
        crate::dynamic::scenarios::basic_auth_dynamic::GROUPED_SCENARIO,
        crate::dynamic::scenarios::rate_limit_local_dynamic::SCENARIO,
        crate::dynamic::scenarios::rate_limit_local_overflow::SCENARIO,
        crate::dynamic::scenarios::request_id_dynamic::SCENARIO,
        crate::dynamic::scenarios::request_access_policy_hmac_dynamic::SCENARIO,
        crate::dynamic::scenarios::route_delegation_dynamic::SCENARIO,
        crate::dynamic::scenarios::grpc_route_delegation_dynamic::SCENARIO,
        crate::dynamic::scenarios::observed_gen_high_freq::SCENARIO,
        crate::dynamic::scenarios::btlsp_ref_flap::SCENARIO,
        crate::dynamic::scenarios::edgion_config_data_ref_flap::SCENARIO,
        crate::dynamic::scenarios::region_route_validation::SCENARIO,
        crate::dynamic::scenarios::sp_backend_change::SCENARIO,
        crate::dynamic::scenarios::sp_weighted::SCENARIO,
        crate::dynamic::scenarios::canary_hot_swap::SCENARIO,
        crate::dynamic::scenarios::dns_resolution_flap::SCENARIO,
        crate::dynamic::scenarios::tcp_esp_reload::SCENARIO,
        crate::dynamic::scenarios::tls_route_attachments::SCENARIO,
        crate::dynamic::scenarios::tls_route_plugins_reload::SCENARIO,
        crate::dynamic::scenarios::gateway_class_params_flap::SCENARIO,
        crate::dynamic::scenarios::edgion_tls_last_known_good::SCENARIO,
        crate::dynamic::scenarios::waf_dynamic::SCENARIO,
        crate::dynamic::scenarios::wasm_runtime_ownership::SCENARIO,
        crate::dynamic::scenarios::l4_timeout_hot_reload::SCENARIO,
    ]
}

/// Look up a scenario by id.
pub fn find(id: &str) -> Option<&'static Scenario> {
    all().into_iter().find(|s| s.id == id)
}
