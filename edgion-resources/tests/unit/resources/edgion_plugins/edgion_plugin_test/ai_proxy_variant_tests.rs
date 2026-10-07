use super::*;
use crate::resources::edgion_plugins::{
    applicable_stages_by_name, AiBackendSelection, AiBackendSelectionConfig, BodyReadFailureMode, OnUnknownModel,
    PluginValidation,
};

fn config(name: &str) -> AiProxyConfig {
    AiProxyConfig {
        api: crate::resources::edgion_plugins::AiApi::OpenaiChat,
        conversion: None,
        usage_observation: crate::resources::edgion_plugins::AiUsageObservation::Provider,
        backend_selection: AiBackendSelectionConfig {
            selection_type: if name.is_empty() {
                AiBackendSelection::LowestLatency
            } else {
                AiBackendSelection::Weighted
            },
        },
        inject_usage_tracking: true,
        on_unknown_model: if name.is_empty() {
            OnUnknownModel::Passthrough
        } else {
            OnUnknownModel::Reject
        },
        model_routes: Vec::new(),
        token_quota: None,
    }
}

#[test]
fn ai_proxy_variant_is_stateless_request_only_and_fail_close_body_gated() {
    let invalid = EdgionPlugin::AiProxy(config(""));
    assert!(invalid.validation_error().is_some());
    assert!(
        invalid.validation_error().is_some(),
        "validation must not depend on a prior call"
    );

    let plugin = EdgionPlugin::AiProxy(config("openai-prod"));
    assert_eq!(plugin.type_name(), "AiProxy");
    assert!(plugin.validation_error().is_none());
    assert!(config("openai-prod").validation_error().is_none());

    let stages = applicable_stages_by_name(plugin.type_name());
    assert!(stages.request);
    assert!(!stages.upstream_response_filter);
    assert!(!stages.upstream_response_body_filter);
    assert!(!stages.upstream_response);

    let requirement = plugin
        .body_requirement(BodyStage::Request)
        .expect("AiProxy intrinsically consumes the request body");
    assert_eq!(requirement.on_read_failure, BodyReadFailureMode::FailClose);
    assert!(plugin.body_requirement(BodyStage::Response).is_none());
}
