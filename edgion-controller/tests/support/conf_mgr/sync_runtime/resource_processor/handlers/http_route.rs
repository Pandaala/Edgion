use super::*;
use edgion_resources::resources::http_route::HTTPBackendRefKind;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg(test)]
pub(super) struct AiProxyAdmissionState {
    pub(super) active_count: usize,
    pub(super) config: Option<AiProxyConfig>,
}

#[cfg(test)]
pub(super) fn ai_proxy_admission_state(
    runtime: &ControllerRuntime,
    namespace: &str,
    name: &str,
) -> Option<AiProxyAdmissionState> {
    let processor = runtime.processor_registry.get("EdgionPlugins")?;
    let key = format!("{namespace}/{name}");
    let plugins = processor.get_cached_any(&key)?.downcast::<EdgionPlugins>().ok()?;
    let mut state = AiProxyAdmissionState::default();
    for entry in plugins
        .spec
        .request_plugins
        .iter()
        .flatten()
        .filter(|entry| entry.is_enabled())
    {
        if let EdgionPlugin::AiProxy(config) = &entry.plugin {
            state.active_count += 1;
            state.config = Some(config.clone());
        }
    }
    Some(state)
}

#[cfg(test)]
pub(super) fn source_ai_admission(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    mut lookup: impl FnMut(&str, &str) -> Option<AiProxyAdmissionState>,
) -> (usize, Vec<AiProxyConfig>, Option<String>) {
    let mut count = 0;
    let mut configs = Vec::new();
    let mut missing = None;
    for filter in rule
        .filters
        .iter()
        .flatten()
        .filter(|filter| filter.filter_type == HTTPRouteFilterType::ExtensionRef)
    {
        let Some(reference) = &filter.extension_ref else {
            continue;
        };
        if !is_supported_edgion_plugins_ref(reference) {
            continue;
        }
        let namespace = route_namespace;
        match lookup(namespace, &reference.name) {
            Some(state) => {
                count += state.active_count;
                if let Some(config) = state.config {
                    configs.push(config);
                }
            }
            None => missing = Some(format!("{namespace}/{}", reference.name)),
        }
    }
    (count, configs, missing)
}

#[cfg(test)]
pub(super) fn protocol_rule_invalid_reason_with_runtime(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    let (active_count, configs, missing) = source_ai_admission(rule, route_namespace, |namespace, name| {
        ai_proxy_admission_state(runtime, namespace, name)
    });
    let configs: Vec<_> = configs.iter().collect();
    // Production parse prepares the sidecar after registering dependencies.
    // Direct status callers still need intrinsic diagnostics from current sources.
    let fallback;
    let resolved = match rule.resolved_ai_admission.as_ref() {
        Some(resolved) => resolved,
        None => {
            fallback = fixture_ai_admission(rule, route_namespace, runtime);
            &fallback
        }
    };
    shared_ai_admission::ai_rule_invalid_reason(
        rule,
        route_namespace,
        Some(resolved),
        AiProxyAdmission {
            active_count,
            configs: &configs,
            missing_reference: missing.as_deref(),
        },
    )
}

#[cfg(test)]
pub(super) fn runtime_ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    protocol_rule_invalid_reason_with_runtime(rule, namespace, runtime)
}

#[cfg(test)]
pub(super) fn backend_variant_invalid_reason(
    rule: &HTTPRouteRule,
    namespace: &str,
    runtime: &ControllerRuntime,
) -> Option<String> {
    shared_ai_admission::backend_variant_invalid_reason(rule, &fixture_ai_admission(rule, namespace, runtime))
}

#[cfg(test)]
pub(super) fn model_route_context_invalid_reason_for_config(
    rule: &HTTPRouteRule,
    namespace: &str,
    runtime: &ControllerRuntime,
    config: &AiProxyConfig,
) -> Option<String> {
    shared_ai_admission::model_route_context_invalid_reason(
        rule,
        namespace,
        &fixture_ai_admission(rule, namespace, runtime),
        config,
    )
}

#[cfg(test)]
pub(super) fn ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    namespace: &str,
    lookup: impl FnMut(&str, &str) -> Option<AiProxyAdmissionState>,
) -> Option<String> {
    let (active_count, configs, missing) = source_ai_admission(rule, namespace, lookup);
    let configs: Vec<_> = configs.iter().collect();
    let facts = ResolvedAiAdmission {
        backends: rule
            .backend_refs
            .iter()
            .flatten()
            .map(|backend| match backend.backend_kind() {
                HTTPBackendRefKind::Service => ResolvedForwardingTarget::Service,
                HTTPBackendRefKind::EdgionBackend => ResolvedForwardingTarget::EdgionBackend {
                    provider: edgion_resources::resources::edgion_backend::AiProvider::OpenAICompatible,
                    has_credential_tpm: false,
                    spec_valid: true,
                    canonical_models: vec![],
                },
                _ => ResolvedForwardingTarget::Missing,
            })
            .collect(),
    };
    shared_ai_admission::ai_rule_invalid_reason(
        rule,
        namespace,
        Some(&facts),
        AiProxyAdmission {
            active_count,
            configs: &configs,
            missing_reference: missing.as_deref(),
        },
    )
}

/// Drive the actual owner-based fact resolver for existing rule-only fixtures.
fn fixture_ai_admission(rule: &HTTPRouteRule, namespace: &str, runtime: &ControllerRuntime) -> ResolvedAiAdmission {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "test-route", "namespace": namespace}, "spec": {}
    }))
    .expect("fixture metadata and an empty optional HTTPRoute spec deserialize");
    route.spec.rules = Some(vec![rule.clone()]);
    populate_ai_admission(&mut route, runtime);
    route
        .spec
        .rules
        .expect("fixture contains its one authored rule")
        .remove(0)
        .resolved_ai_admission
        .expect("populate_ai_admission sets every authored rule sidecar")
}
