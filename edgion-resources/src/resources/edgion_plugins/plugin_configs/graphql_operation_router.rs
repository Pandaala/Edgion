//! Exact operation routing restricted to declared HTTPRoute Service backends.

use super::dynamic_internal_upstream::DynUpstreamTarget;
use crate::graphql_request::GraphqlOperationType;
use crate::resources::common::kubernetes_name::{is_dns_label, is_dns_subdomain};
use crate::resources::edgion_plugins::PluginValidation;
use crate::resources::http_route::{HTTPBackendRefKind, HTTPRouteRule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlOperationRouterConfig {
    pub rules: Vec<GraphqlOperationRouteRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_backend_ref: Option<DynUpstreamTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlOperationRouteRule {
    pub operation_type: GraphqlOperationType,
    pub operation_name: String,
    pub backend_ref: DynUpstreamTarget,
}

impl GraphqlOperationRouterConfig {
    pub fn targets(&self) -> impl Iterator<Item = &DynUpstreamTarget> {
        self.rules
            .iter()
            .map(|r| &r.backend_ref)
            .chain(self.default_backend_ref.iter())
    }
}

impl PluginValidation for GraphqlOperationRouterConfig {
    fn validation_error(&self) -> Option<String> {
        if self.rules.is_empty() || self.rules.len() > 256 {
            return Some("rules must contain 1..256 operation mappings".into());
        }
        let mut keys = HashSet::new();
        for rule in &self.rules {
            let bytes = rule.operation_name.as_bytes();
            if bytes.is_empty()
                || bytes.len() > 256
                || !bytes[0].is_ascii_alphabetic() && bytes[0] != b'_'
                || !bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                return Some("operationName must be a GraphQL name of at most 256 bytes".into());
            }
            if !keys.insert((rule.operation_type, rule.operation_name.as_str())) {
                return Some("duplicate operation type/name mapping".into());
            }
        }
        for target in self.targets() {
            if !is_dns_subdomain(&target.name, 253) || target.namespace.as_ref().is_some_and(|n| !is_dns_label(n, 63)) {
                return Some("backendRef must have a valid bounded name and namespace".into());
            }
        }
        None
    }
}

/// Cold structural membership check, shared by Controller and scoped publisher.
/// Runtime authorization still uses the compiled canonical named-jump matcher.
pub fn graphql_router_binding_error(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    configs: &[&GraphqlOperationRouterConfig],
    delegated: bool,
) -> Option<String> {
    if configs.is_empty() {
        return None;
    }
    if delegated {
        return Some("GraphqlOperationRouter does not support HTTPRoute delegation".into());
    }
    let mut declared = HashSet::new();
    for backend in rule.backend_refs.iter().flatten() {
        if backend.backend_kind() != HTTPBackendRefKind::Service {
            continue;
        }
        if !declared.insert((
            backend.namespace.as_deref().unwrap_or(route_namespace),
            backend.name.as_str(),
        )) {
            return Some("GraphqlOperationRouter requires unique Service namespace/name keys".into());
        }
    }
    for config in configs {
        for target in config.targets() {
            if !declared.contains(&(
                target.namespace.as_deref().unwrap_or(route_namespace),
                target.name.as_str(),
            )) {
                return Some(
                    "GraphqlOperationRouter backendRef is not declared in the attaching HTTPRoute rule".into(),
                );
            }
        }
    }
    None
}

/// Validate effective executable composition after structural binding.
pub fn graphql_router_attachment_error(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    configs: &[&GraphqlOperationRouterConfig],
    delegated: bool,
    jump_publishers: usize,
    incompatible_owner: bool,
) -> Option<String> {
    if let Some(error) = graphql_router_binding_error(rule, route_namespace, configs, delegated) {
        return Some(error);
    }
    if configs.is_empty() {
        return None;
    }
    if configs.len() != 1 || jump_publishers != 1 || incompatible_owner {
        return Some("GraphqlOperationRouter requires exclusive backend selection".into());
    }
    configs.iter().find_map(|config| config.validation_error())
}

/// Inspect source-enabled router bindings independently of policy masks.
pub fn graphql_router_rule_binding_error(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    plugins: &[&crate::EdgionPlugins],
    delegated: bool,
) -> Option<String> {
    use crate::resources::edgion_plugins::EdgionPlugin;
    let configs: Vec<_> = plugins
        .iter()
        .flat_map(|plugin| plugin.spec.request_plugins.iter().flatten())
        .filter(|entry| entry.is_enabled())
        .filter_map(|entry| match &entry.plugin {
            EdgionPlugin::GraphqlOperationRouter(config) => Some(config),
            _ => None,
        })
        .collect();
    graphql_router_binding_error(rule, route_namespace, &configs, delegated)
}

/// Apply the same rule contract to raw desired or effective scoped plugin views.
pub fn graphql_router_rule_error(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    plugins: &[&crate::EdgionPlugins],
    delegated: bool,
) -> Option<String> {
    use crate::resources::edgion_plugins::EdgionPlugin;
    let entries: Vec<_> = plugins
        .iter()
        .flat_map(|p| p.spec.request_plugins.iter().flatten())
        .filter(|e| e.is_enabled())
        .collect();
    let configs: Vec<_> = entries
        .iter()
        .filter_map(|e| match &e.plugin {
            EdgionPlugin::GraphqlOperationRouter(c) => Some(c),
            _ => None,
        })
        .collect();
    let publishers = entries.iter().filter(|e| e.plugin.publishes_upstream_jump()).count();
    let incompatible = entries.iter().any(|e| {
        matches!(
            e.plugin,
            EdgionPlugin::AiProxy(_)
                | EdgionPlugin::AiGuard(_)
                | EdgionPlugin::Guardrail(_)
                | EdgionPlugin::GrpcJsonTranscoder(_)
        )
    });
    graphql_router_attachment_error(rule, route_namespace, &configs, delegated, publishers, incompatible)
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/graphql_operation_router_test.rs"]
mod tests;
