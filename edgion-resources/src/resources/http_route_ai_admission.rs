//! Pure HTTP AI admission shared by Controller source diagnostics and scoped Gateway publication.
//!
//! Resolved facts describe declarations, not executable backend availability. All
//! checks run on configuration publication; requests consume the resulting gate.

use super::edgion_plugins::AiProxyConfig;
use super::http_route::{
    logical_backend_slot_identities, HTTPBackendRef, HTTPBackendRefKind, HTTPRouteFilterType, HTTPRouteRule,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Controller-owned facts aligned with every original backendRef slot.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAiAdmission {
    pub backends: Vec<ResolvedForwardingTarget>,
}

/// Only source validity and canonical model declarations cross the transport.
/// Credentials, endpoints and runtime compilation results are deliberately absent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ResolvedForwardingTarget {
    Missing,
    Service,
    EdgionBackend {
        #[serde(rename = "specValid")]
        spec_valid: bool,
        #[serde(rename = "canonicalModels")]
        canonical_models: Vec<String>,
    },
    Incompatible,
}

/// Borrowed effective plugin inputs. The Gateway supplies its selected scoped
/// generation; the Controller supplies enabled source entries without policy.
pub struct AiProxyAdmission<'a> {
    pub active_count: usize,
    pub configs: &'a [&'a AiProxyConfig],
    pub missing_reference: Option<&'a str>,
}

fn facts_match_rule(rule: &HTTPRouteRule, resolved: &ResolvedAiAdmission) -> bool {
    let backends = rule.backend_refs.as_deref().unwrap_or_default();
    backends.len() == resolved.backends.len()
        && backends
            .iter()
            .zip(&resolved.backends)
            .all(|(backend, fact)| match backend.backend_kind() {
                HTTPBackendRefKind::Service => matches!(
                    fact,
                    ResolvedForwardingTarget::Service | ResolvedForwardingTarget::Missing
                ),
                HTTPBackendRefKind::EdgionBackend => !matches!(fact, ResolvedForwardingTarget::Service),
                HTTPBackendRefKind::RouteDelegation | HTTPBackendRefKind::Unsupported => {
                    matches!(fact, ResolvedForwardingTarget::Missing)
                }
            })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForwardingBackendClass {
    Service,
    Ai,
    Missing,
    Denied,
    Invalid,
}

fn forwarding_class(backend: &HTTPBackendRef, fact: &ResolvedForwardingTarget) -> ForwardingBackendClass {
    if backend.ref_denied.is_some() {
        return ForwardingBackendClass::Denied;
    }
    match backend.backend_kind() {
        HTTPBackendRefKind::Service => {
            if matches!(fact, ResolvedForwardingTarget::Service) {
                ForwardingBackendClass::Service
            } else {
                ForwardingBackendClass::Missing
            }
        }
        HTTPBackendRefKind::EdgionBackend if backend.port.is_some() => ForwardingBackendClass::Invalid,
        HTTPBackendRefKind::EdgionBackend => match fact {
            ResolvedForwardingTarget::EdgionBackend { spec_valid: true, .. } => ForwardingBackendClass::Ai,
            ResolvedForwardingTarget::EdgionBackend { spec_valid: false, .. } => ForwardingBackendClass::Invalid,
            _ => ForwardingBackendClass::Missing,
        },
        HTTPBackendRefKind::RouteDelegation | HTTPBackendRefKind::Unsupported => ForwardingBackendClass::Invalid,
    }
}

/// Check the complete ordered contract. Policy Block is composed before this
/// result by the Gateway publisher; it never changes these source facts.
pub fn ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    resolved: Option<&ResolvedAiAdmission>,
    proxies: AiProxyAdmission<'_>,
) -> Option<String> {
    let requires_facts = proxies.active_count > 0
        || rule
            .backend_refs
            .iter()
            .flatten()
            .any(|backend| backend.backend_kind() != HTTPBackendRefKind::Service);
    let Some(resolved) = resolved.filter(|facts| facts_match_rule(rule, facts)) else {
        return requires_facts.then(|| "AI routing admission metadata is unavailable".to_string());
    };
    runtime_ai_rule_invalid_reason(rule, route_namespace, resolved, &proxies)
        .or_else(|| backend_variant_invalid_reason(rule, resolved))
        .or_else(|| {
            proxies
                .configs
                .first()
                .and_then(|config| model_route_context_invalid_reason(rule, route_namespace, resolved, config))
        })
}

pub fn runtime_ai_rule_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    resolved: &ResolvedAiAdmission,
    proxies: &AiProxyAdmission<'_>,
) -> Option<String> {
    if resolved.backends.len() != rule.backend_refs.as_ref().map_or(0, Vec::len) {
        return Some("AI routing admission metadata is unavailable".to_string());
    }
    let refs = rule.backend_refs.as_deref().unwrap_or_default();
    let identities = logical_backend_slot_identities(refs, route_namespace);
    let mut promoted_slots = HashSet::new();
    for config in proxies.configs {
        for model_route in &config.model_routes {
            for candidate in model_route.action.candidates.iter().flatten() {
                let matches = identities
                    .iter()
                    .enumerate()
                    .filter(|(_, identity)| identity.matches_ai_selector(&candidate.backend))
                    .map(|(slot, _)| slot)
                    .collect::<Vec<_>>();
                if let [slot] = matches.as_slice() {
                    if candidate.weight.unwrap_or(refs[*slot].weight.unwrap_or(1)) > 0 {
                        promoted_slots.insert(*slot);
                    }
                }
            }
        }
    }

    let classes: Vec<_> = refs
        .iter()
        .enumerate()
        .filter(|(slot, backend)| backend.weight.unwrap_or(1) > 0 || promoted_slots.contains(slot))
        .map(|(slot, backend)| (slot, backend, forwarding_class(backend, &resolved.backends[slot])))
        .collect();

    let has_ai = classes.iter().any(|(_, _, class)| *class == ForwardingBackendClass::Ai);
    if !has_ai {
        if classes
            .iter()
            .any(|(_, _, class)| *class == ForwardingBackendClass::Invalid)
        {
            return Some("Forwarding backend has an invalid kind or variant".to_string());
        }
        return (proxies.active_count > 0)
            .then(|| "AiProxy requires a rule with positive AI EdgionBackend slots".to_string());
    }

    if rule
        .retry
        .as_ref()
        .and_then(|retry| retry.attempts)
        .is_some_and(|attempts| attempts > 7)
    {
        return Some("AI routing retry.attempts must not exceed 7".to_string());
    }
    if classes
        .iter()
        .any(|(_, _, class)| matches!(class, ForwardingBackendClass::Service | ForwardingBackendClass::Invalid))
    {
        return Some("AI EdgionBackend slots must not be mixed with other forwarding backend variants".to_string());
    }
    for (_, backend_ref, class) in &classes {
        if *class != ForwardingBackendClass::Ai {
            continue;
        }
        if backend_ref.port.is_some() {
            return Some("EdgionBackend backendRefs must not set port".to_string());
        }
        if let Some(filter) = backend_ref.filters.iter().flatten().find(|filter| {
            !matches!(
                filter.filter_type,
                HTTPRouteFilterType::RequestHeaderModifier | HTTPRouteFilterType::ResponseHeaderModifier
            )
        }) {
            return Some(format!(
                "EdgionBackend backendRefs do not support {:?} filters",
                filter.filter_type
            ));
        }
    }
    if let Some(key) = proxies.missing_reference {
        return Some(format!(
            "AI routing requires exactly one enabled AiProxy, but EdgionPlugins '{key}' was not found"
        ));
    }
    (proxies.active_count != 1).then(|| {
        format!(
            "AI routing requires exactly one enabled AiProxy (found {})",
            proxies.active_count
        )
    })
}

pub fn backend_variant_invalid_reason(rule: &HTTPRouteRule, resolved: &ResolvedAiAdmission) -> Option<String> {
    if resolved.backends.len() != rule.backend_refs.as_ref().map_or(0, Vec::len) {
        return Some("AI routing admission metadata is unavailable".to_string());
    }
    for (slot, _) in rule.backend_refs.iter().flatten().enumerate().filter(|(_, backend)| {
        backend.weight.unwrap_or(1) > 0
            && backend.ref_denied.is_none()
            && backend.backend_kind() == HTTPBackendRefKind::EdgionBackend
    }) {
        match &resolved.backends[slot] {
            ResolvedForwardingTarget::Incompatible => {
                return Some("EdgionBackend target has an incompatible cached representation".to_string())
            }
            ResolvedForwardingTarget::EdgionBackend { spec_valid: false, .. } => {
                return Some("EdgionBackend target does not configure a supported variant".to_string())
            }
            _ => {}
        }
    }
    None
}

pub fn model_route_context_invalid_reason(
    rule: &HTTPRouteRule,
    route_namespace: &str,
    resolved: &ResolvedAiAdmission,
    config: &AiProxyConfig,
) -> Option<String> {
    if resolved.backends.len() != rule.backend_refs.as_ref().map_or(0, Vec::len) {
        return Some("AI routing admission metadata is unavailable".to_string());
    }
    if config.model_routes.is_empty() {
        return None;
    }
    let refs = rule.backend_refs.as_deref().unwrap_or_default();
    let identities = logical_backend_slot_identities(refs, route_namespace);

    for (route_index, model_route) in config.model_routes.iter().enumerate() {
        let selected_slots = if let Some(candidates) = &model_route.action.candidates {
            let mut slots = Vec::with_capacity(candidates.len());
            for (candidate_index, candidate) in candidates.iter().enumerate() {
                let matches = identities
                    .iter()
                    .enumerate()
                    .filter(|(_, identity)| identity.matches_ai_selector(&candidate.backend))
                    .map(|(slot, _)| slot)
                    .collect::<Vec<_>>();
                let [slot] = matches.as_slice() else {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}] must resolve to one backendRef slot"
                    ));
                };
                let backend_ref = &refs[*slot];
                if backend_ref.ref_denied.is_some() {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}] selects a denied backendRef"
                    ));
                }
                if backend_ref.backend_kind() != HTTPBackendRefKind::EdgionBackend || backend_ref.port.is_some() {
                    return Some(format!(
                        "modelRoutes[{route_index}].action.candidates[{candidate_index}] must select an EdgionBackend slot"
                    ));
                }
                slots.push((*slot, candidate.weight.unwrap_or(backend_ref.weight.unwrap_or(1))));
            }
            slots
        } else {
            refs.iter()
                .enumerate()
                .filter(|(_, backend_ref)| {
                    backend_ref.ref_denied.is_none()
                        && backend_ref.backend_kind() == HTTPBackendRefKind::EdgionBackend
                        && backend_ref.port.is_none()
                })
                .map(|(slot, backend_ref)| (slot, backend_ref.weight.unwrap_or(1)))
                .collect()
        };

        let mut existing_selected = false;
        let mut rewrite_supported = model_route.action.rewrite.is_none();
        for (slot, effective_weight) in selected_slots {
            let canonical_models = match &resolved.backends[slot] {
                ResolvedForwardingTarget::EdgionBackend { canonical_models, .. } => canonical_models,
                ResolvedForwardingTarget::Incompatible => {
                    return Some("modelRoutes candidate has an incompatible cached backend representation".to_string())
                }
                ResolvedForwardingTarget::Missing | ResolvedForwardingTarget::Service => continue,
            };
            if effective_weight <= 0 {
                continue;
            }
            existing_selected = true;
            if let Some(rewrite) = &model_route.action.rewrite {
                rewrite_supported |= canonical_models.iter().any(|model| model == rewrite);
            }
        }
        if existing_selected && !rewrite_supported {
            return Some(format!(
                "modelRoutes[{route_index}].action.rewrite must name a canonical model on a selected backend"
            ));
        }
    }
    None
}

#[cfg(test)]
#[path = "../../tests/unit/resources/http_route_ai_admission_test.rs"]
mod tests;
