// Scenario registry. To add a new scenario, append a single entry to this list.

use crate::dynamic::scenario::Scenario;

/// All registered dynamic scenarios.
pub fn all() -> Vec<&'static Scenario> {
    vec![
        crate::dynamic::scenarios::gateway_dynamic::SCENARIO,
        crate::dynamic::scenarios::dsl_dynamic::SCENARIO,
        crate::dynamic::scenarios::basic_auth_dynamic::SCENARIO,
        crate::dynamic::scenarios::basic_auth_dynamic::GROUPED_SCENARIO,
        crate::dynamic::scenarios::rate_limit_dynamic::SCENARIO,
        crate::dynamic::scenarios::route_delegation_dynamic::SCENARIO,
        crate::dynamic::scenarios::grpc_route_delegation_dynamic::SCENARIO,
        crate::dynamic::scenarios::observed_gen_high_freq::SCENARIO,
        crate::dynamic::scenarios::btlsp_ref_flap::SCENARIO,
        crate::dynamic::scenarios::edgion_config_data_ref_flap::SCENARIO,
        crate::dynamic::scenarios::sp_backend_change::SCENARIO,
        crate::dynamic::scenarios::sp_weighted::SCENARIO,
        crate::dynamic::scenarios::canary_hot_swap::SCENARIO,
        crate::dynamic::scenarios::dns_resolution_flap::SCENARIO,
        crate::dynamic::scenarios::tcp_esp_reload::SCENARIO,
        crate::dynamic::scenarios::gateway_class_params_flap::SCENARIO,
        crate::dynamic::scenarios::edgion_tls_last_known_good::SCENARIO,
    ]
}

/// Look up a scenario by id.
pub fn find(id: &str) -> Option<&'static Scenario> {
    all().into_iter().find(|s| s.id == id)
}
