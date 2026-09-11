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
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(backends: serde_json::Value) -> HTTPRouteRule {
        serde_json::from_value(json!({"backendRefs": backends})).unwrap()
    }

    fn ai_fact(valid: bool) -> ResolvedForwardingTarget {
        ResolvedForwardingTarget::EdgionBackend {
            spec_valid: valid,
            canonical_models: vec!["canonical".to_string()],
        }
    }

    fn check(
        rule: &HTTPRouteRule,
        facts: Option<&ResolvedAiAdmission>,
        count: usize,
        config: Option<&AiProxyConfig>,
    ) -> Option<String> {
        let configs: Vec<_> = config.into_iter().collect();
        ai_rule_invalid_reason(
            rule,
            "apps",
            facts,
            AiProxyAdmission {
                active_count: count,
                configs: &configs,
                missing_reference: None,
            },
        )
    }

    #[test]
    fn required_metadata_never_becomes_ordinary_forwarding() {
        let ai = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
        for facts in [
            None,
            Some(ResolvedAiAdmission { backends: vec![] }),
            Some(ResolvedAiAdmission {
                backends: vec![ResolvedForwardingTarget::Service],
            }),
        ] {
            assert_eq!(
                check(&ai, facts.as_ref(), 0, None).as_deref(),
                Some("AI routing admission metadata is unavailable")
            );
        }
        let service = rule(json!([{"name": "web", "port": 80}]));
        assert_eq!(check(&service, None, 0, None), None);
        assert!(check(&service, None, 1, None).is_some());
        let unsupported = rule(json!([{"group": "edgion.io", "kind": "Unsupported", "name": "target"}]));
        assert!(check(&unsupported, None, 0, None).is_some());
    }

    #[test]
    fn bypassed_ai_proxy_rejects_the_matched_ai_rule() {
        let rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
        let facts = ResolvedAiAdmission {
            backends: vec![ai_fact(true)],
        };
        assert_eq!(check(&rule, Some(&facts), 1, None), None);
        assert_eq!(
            check(&rule, Some(&facts), 0, None).as_deref(),
            Some("AI routing requires exactly one enabled AiProxy (found 0)")
        );
        assert_eq!(
            check(&rule, Some(&facts), 2, None).as_deref(),
            Some("AI routing requires exactly one enabled AiProxy (found 2)")
        );
    }

    #[test]
    fn promoted_zero_weight_slots_use_current_effective_config() {
        let rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models", "weight": 0}]));
        let config: AiProxyConfig = serde_json::from_value(json!({
            "modelRoutes": [{"match": {"model": "virtual"}, "action": {
                "candidates": [{"backend": {"group": "edgion.io", "kind": "EdgionBackend", "namespace": "apps", "name": "models", "occurrence": 0}, "weight": 1}]
            }}]
        })).unwrap();
        let facts = ResolvedAiAdmission {
            backends: vec![ai_fact(true)],
        };
        assert!(check(&rule, Some(&facts), 1, None).is_some());
        assert_eq!(check(&rule, Some(&facts), 1, Some(&config)), None);
        let invalid = ResolvedAiAdmission {
            backends: vec![ai_fact(false)],
        };
        assert_eq!(
            check(&rule, Some(&invalid), 1, Some(&config)).as_deref(),
            Some("Forwarding backend has an invalid kind or variant")
        );
    }

    #[test]
    fn ordered_shape_variant_and_model_failures_preserve_diagnostics() {
        let mut rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
        let facts = ResolvedAiAdmission {
            backends: vec![ai_fact(true)],
        };
        let config: AiProxyConfig = serde_json::from_value(json!({
            "modelRoutes": [{"match": {"model": "virtual"}, "action": {"rewrite": "missing"}}]
        }))
        .unwrap();
        assert_eq!(
            check(&rule, Some(&facts), 1, Some(&config)).as_deref(),
            Some("modelRoutes[0].action.rewrite must name a canonical model on a selected backend")
        );
        rule.retry = serde_json::from_value(json!({"attempts": 8})).ok();
        assert_eq!(
            check(&rule, Some(&facts), 1, Some(&config)).as_deref(),
            Some("AI routing retry.attempts must not exceed 7")
        );
        let invalid = ResolvedAiAdmission {
            backends: vec![ai_fact(false)],
        };
        assert_eq!(
            backend_variant_invalid_reason(&rule, &invalid).as_deref(),
            Some("EdgionBackend target does not configure a supported variant")
        );
        let incompatible = ResolvedAiAdmission {
            backends: vec![ResolvedForwardingTarget::Incompatible],
        };
        assert_eq!(
            backend_variant_invalid_reason(&rule, &incompatible).as_deref(),
            Some("EdgionBackend target has an incompatible cached representation")
        );
    }

    #[test]
    fn missing_and_denied_slots_do_not_become_service_backends() {
        let mut rule = rule(json!([
            {"group": "edgion.io", "kind": "EdgionBackend", "name": "models"},
            {"name": "web", "port": 80}
        ]));
        let mut facts = ResolvedAiAdmission {
            backends: vec![ai_fact(true), ResolvedForwardingTarget::Missing],
        };
        assert_eq!(check(&rule, Some(&facts), 1, None), None);
        facts.backends[1] = ResolvedForwardingTarget::Service;
        assert!(check(&rule, Some(&facts), 1, None)
            .unwrap()
            .contains("must not be mixed"));
        rule.backend_refs.as_mut().unwrap()[1].ref_denied = Some(super::super::common::RefDenied {
            target_namespace: "other".to_string(),
            target_name: "web".to_string(),
            reason: None,
        });
        assert_eq!(check(&rule, Some(&facts), 1, None), None);
    }

    #[test]
    fn facts_survive_clone_and_transport_without_entering_operator_schema() {
        let mut rule = rule(json!([{"group": "edgion.io", "kind": "EdgionBackend", "name": "models"}]));
        rule.resolved_ai_admission = Some(ResolvedAiAdmission {
            backends: vec![ai_fact(true)],
        });
        let clone = rule.clone();
        assert_eq!(clone.resolved_ai_admission, rule.resolved_ai_admission);
        let wire = serde_json::to_value(&clone).unwrap();
        assert!(wire.get("resolvedAiAdmission").is_some());
        let restored: HTTPRouteRule = serde_json::from_value(wire).unwrap();
        assert_eq!(restored.resolved_ai_admission, rule.resolved_ai_admission);
        let schema = serde_json::to_value(schemars::schema_for!(HTTPRouteRule)).unwrap();
        assert!(!schema["properties"]
            .as_object()
            .unwrap()
            .contains_key("resolvedAiAdmission"));
    }
}
