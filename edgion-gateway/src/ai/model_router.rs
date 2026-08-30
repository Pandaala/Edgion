//! Bounded model/header routing transform.
//!
//! This module never samples weights or selects a backend. It resolves one
//! winning structured rule and derives effective weights for the existing AI
//! conformance gate and scheduler.

use std::collections::HashMap;

use edgion_resources::resources::edgion_plugins::{
    AiBackendSlotSelector, AiModelPattern, AiModelRoute, AiModelRouteAction,
};

use crate::routes::http::match_unit::CompiledBackend;

#[derive(Clone, Debug)]
struct CompiledModelRoute {
    source_pattern: String,
    pattern: AiModelPattern,
    headers: Vec<(String, String)>,
    action: AiModelRouteAction,
}

#[derive(Clone, Debug, Default)]
pub struct ModelRouteTable {
    routes: Vec<CompiledModelRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRouteDecision {
    pub matched: bool,
    pub identity_model: String,
    /// Effective weights indexed by raw backend declaration slot.
    pub weights: Vec<i32>,
    /// A matched rewrite must resolve as a canonical catalog name and cannot
    /// use unknown-model passthrough.
    pub rewrite_requires_canonical: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelRouteDecisionError {
    InvalidSelector,
    InvalidWeight,
}

impl ModelRouteTable {
    pub fn compile(routes: &[AiModelRoute]) -> Option<Self> {
        let routes = routes
            .iter()
            .map(|route| {
                Some(CompiledModelRoute {
                    source_pattern: route.match_.model.clone(),
                    pattern: AiModelPattern::parse(&route.match_.model).ok()?,
                    headers: route
                        .match_
                        .headers
                        .iter()
                        .map(|header| (header.name.to_ascii_lowercase(), header.value.clone()))
                        .collect(),
                    action: route.action.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { routes })
    }

    pub fn decide(
        &self,
        requested_model: &str,
        request_headers: &[(String, String)],
        backends: &[CompiledBackend],
    ) -> Result<ModelRouteDecision, ModelRouteDecisionError> {
        let winner = self
            .routes
            .iter()
            .filter(|route| {
                route.pattern.matches(requested_model)
                    && route.headers.iter().all(|(name, expected)| {
                        request_headers.iter().any(|(actual_name, actual_value)| {
                            actual_name.eq_ignore_ascii_case(name) && actual_value == expected
                        })
                    })
            })
            .max_by_key(|route| {
                (
                    route.pattern.exactness(),
                    route.pattern.literal_len(),
                    route.headers.len(),
                )
            });

        let Some(winner) = winner else {
            return Ok(ModelRouteDecision {
                matched: false,
                identity_model: requested_model.to_string(),
                weights: backends
                    .iter()
                    .map(|backend| backend.schema.weight.unwrap_or(1))
                    .collect(),
                rewrite_requires_canonical: false,
            });
        };

        let mut weights: Vec<i32> = backends
            .iter()
            .map(|backend| backend.schema.weight.unwrap_or(1))
            .collect();
        if let Some(candidates) = &winner.action.candidates {
            let overrides: HashMap<usize, Option<i32>> = candidates
                .iter()
                .map(|candidate| {
                    resolve_selector(&candidate.backend, backends)
                        .map(|slot| (slot, candidate.weight))
                        .ok_or(ModelRouteDecisionError::InvalidSelector)
                })
                .collect::<Result<_, _>>()?;
            for backend in backends {
                if backend.is_permitted_ai_backend() {
                    weights[backend.slot] = match overrides.get(&backend.slot) {
                        Some(Some(weight)) if *weight >= 0 => *weight,
                        Some(Some(_)) => return Err(ModelRouteDecisionError::InvalidWeight),
                        Some(None) => backend.schema.weight.unwrap_or(1),
                        None => 0,
                    };
                }
            }
        }
        Ok(ModelRouteDecision {
            matched: true,
            identity_model: winner
                .action
                .rewrite
                .clone()
                .unwrap_or_else(|| requested_model.to_string()),
            weights,
            rewrite_requires_canonical: winner.action.rewrite.is_some(),
        })
    }

    pub fn header_applicable_patterns<'a>(
        &'a self,
        request_headers: &'a [(String, String)],
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.routes.iter().filter_map(move |route| {
            route
                .headers
                .iter()
                .all(|(name, expected)| {
                    request_headers.iter().any(|(actual_name, actual_value)| {
                        actual_name.eq_ignore_ascii_case(name) && actual_value == expected
                    })
                })
                .then_some(route.source_pattern.as_str())
        })
    }

    /// Conservative family-listing guard: a wildcard family is omitted when a
    /// different header-applicable route overlaps any part of its prefix
    /// language. Listing must not claim that the complete family is reachable.
    pub fn wildcard_is_partially_shadowed(&self, wildcard: &str, request_headers: &[(String, String)]) -> bool {
        let Some(prefix) = wildcard.strip_suffix('*') else {
            return false;
        };
        let header_applicable = |route: &&CompiledModelRoute| {
            route.headers.iter().all(|(name, expected)| {
                request_headers.iter().any(|(actual_name, actual_value)| {
                    actual_name.eq_ignore_ascii_case(name) && actual_value == expected
                })
            })
        };
        let has_identical_route = self
            .routes
            .iter()
            .filter(header_applicable)
            .any(|route| route.source_pattern == wildcard);
        self.routes.iter().any(|route| {
            if route.source_pattern == wildcard || !header_applicable(&route) {
                return false;
            }
            match &route.pattern {
                AiModelPattern::Exact(exact) => exact.starts_with(prefix),
                AiModelPattern::Prefix(other) => {
                    other.starts_with(prefix) || (!has_identical_route && prefix.starts_with(other))
                }
            }
        })
    }
}

fn resolve_selector(selector: &AiBackendSlotSelector, backends: &[CompiledBackend]) -> Option<usize> {
    let mut matching = backends
        .iter()
        .filter(|backend| backend.identity.matches_ai_selector(selector));
    let slot = matching.next()?.slot;
    matching.next().is_none().then_some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::PluginRuntime;
    use edgion_resources::resources::edgion_plugins::{AiModelHeaderMatch, AiModelRouteCandidate, AiModelRouteMatch};
    use edgion_resources::resources::http_route::HTTPBackendRef;
    use std::sync::Arc;

    fn backend(name: &str, slot: usize, weight: i32) -> CompiledBackend {
        CompiledBackend::compile(
            HTTPBackendRef {
                name: name.to_string(),
                namespace: Some("ai".to_string()),
                port: None,
                weight: Some(weight),
                group: Some("edgion.io".to_string()),
                kind: Some("EdgionBackend".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: None,
            },
            "ai",
            slot,
            Arc::new(PluginRuntime::new()),
        )
    }

    fn selector(name: &str) -> AiBackendSlotSelector {
        AiBackendSlotSelector {
            group: "edgion.io".to_string(),
            kind: "EdgionBackend".to_string(),
            namespace: "ai".to_string(),
            name: name.to_string(),
            section: None,
            occurrence: 0,
        }
    }

    #[test]
    fn exact_and_headers_beat_wildcard_and_apply_one_transform() {
        let routes = vec![
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "gpt*".to_string(),
                    headers: Vec::new(),
                },
                action: AiModelRouteAction {
                    rewrite: Some("broad".to_string()),
                    candidates: None,
                },
            },
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "gpt-4".to_string(),
                    headers: vec![AiModelHeaderMatch {
                        name: "X-Tier".to_string(),
                        value: "paid".to_string(),
                    }],
                },
                action: AiModelRouteAction {
                    rewrite: Some("exact".to_string()),
                    candidates: Some(vec![AiModelRouteCandidate {
                        backend: selector("b"),
                        weight: Some(9),
                    }]),
                },
            },
        ];
        let table = ModelRouteTable::compile(&routes).unwrap();
        let decision = table
            .decide(
                "gpt-4",
                &[("x-tier".to_string(), "paid".to_string())],
                &[backend("a", 0, 1), backend("b", 1, 3)],
            )
            .unwrap();
        assert_eq!(decision.identity_model, "exact");
        assert_eq!(decision.weights, vec![0, 9]);
        assert!(decision.rewrite_requires_canonical);
    }

    #[test]
    fn wildcard_family_listing_detects_partial_shadowing() {
        let table = ModelRouteTable::compile(&[AiModelRoute {
            match_: AiModelRouteMatch {
                model: "gpt-pro*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: Some("pro".to_string()),
                candidates: None,
            },
        }])
        .unwrap();
        assert!(table.wildcard_is_partially_shadowed("gpt*", &[]));
        assert!(!table.wildcard_is_partially_shadowed("other*", &[]));
        assert!(!table.wildcard_is_partially_shadowed("gpt-pro*", &[]));

        let with_broader = ModelRouteTable::compile(&[
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "gpt*".to_string(),
                    headers: Vec::new(),
                },
                action: AiModelRouteAction {
                    rewrite: Some("broad".to_string()),
                    candidates: None,
                },
            },
            AiModelRoute {
                match_: AiModelRouteMatch {
                    model: "gpt-pro*".to_string(),
                    headers: Vec::new(),
                },
                action: AiModelRouteAction {
                    rewrite: Some("pro".to_string()),
                    candidates: None,
                },
            },
        ])
        .unwrap();
        assert!(!with_broader.wildcard_is_partially_shadowed("gpt-pro*", &[]));
        assert!(with_broader.wildcard_is_partially_shadowed("gpt*", &[]));
    }

    #[test]
    fn specificity_is_exact_then_longest_prefix_then_header_count() {
        let route = |model: &str, headers: Vec<AiModelHeaderMatch>, rewrite: &str| AiModelRoute {
            match_: AiModelRouteMatch {
                model: model.to_string(),
                headers,
            },
            action: AiModelRouteAction {
                rewrite: Some(rewrite.to_string()),
                candidates: None,
            },
        };
        let header = |name: &str, value: &str| AiModelHeaderMatch {
            name: name.to_string(),
            value: value.to_string(),
        };
        let table = ModelRouteTable::compile(&[
            route("g*", Vec::new(), "broad"),
            route("gpt*", Vec::new(), "long"),
            route("gpt*", vec![header("x-tier", "paid")], "headed"),
            route("gpt-4", Vec::new(), "exact"),
        ])
        .unwrap();
        let backends = [backend("a", 0, 1)];
        assert_eq!(table.decide("gpt-3", &[], &backends).unwrap().identity_model, "long");
        assert_eq!(
            table
                .decide(
                    "gpt-3",
                    &[
                        ("X-Tier".to_string(), "free".to_string()),
                        ("x-tier".to_string(), "paid".to_string()),
                    ],
                    &backends,
                )
                .unwrap()
                .identity_model,
            "headed"
        );
        assert_eq!(table.decide("gpt-4", &[], &backends).unwrap().identity_model, "exact");
        assert!(!table.decide("GPT-4", &[], &backends).unwrap().matched);
        assert!(!table.decide("other", &[], &backends).unwrap().matched);
    }

    #[test]
    fn typed_occurrence_and_weight_overrides_preserve_unselected_invalid_share() {
        let mut first = backend("same", 0, 7);
        let mut second = backend("same", 1, 0);
        second.identity.occurrence = 1;
        first.schema.ref_denied = Some(edgion_resources::resources::common::RefDenied {
            target_namespace: "ai".to_string(),
            target_name: "same".to_string(),
            reason: Some("denied".to_string()),
        });
        let mut second_selector = selector("same");
        second_selector.occurrence = 1;
        let table = ModelRouteTable::compile(&[AiModelRoute {
            match_: AiModelRouteMatch {
                model: "virtual".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: None,
                candidates: Some(vec![AiModelRouteCandidate {
                    backend: second_selector,
                    weight: Some(5),
                }]),
            },
        }])
        .unwrap();
        let decision = table.decide("virtual", &[], &[first, second]).unwrap();
        assert_eq!(decision.weights, vec![7, 5]);

        let missing = ModelRouteTable::compile(&[AiModelRoute {
            match_: AiModelRouteMatch {
                model: "virtual".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: None,
                candidates: Some(vec![AiModelRouteCandidate {
                    backend: selector("missing"),
                    weight: None,
                }]),
            },
        }])
        .unwrap();
        assert_eq!(
            missing.decide("virtual", &[], &[backend("same", 0, 1)]),
            Err(ModelRouteDecisionError::InvalidSelector)
        );

        let all_zero = ModelRouteTable::compile(&[AiModelRoute {
            match_: AiModelRouteMatch {
                model: "virtual".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: None,
                candidates: Some(vec![AiModelRouteCandidate {
                    backend: selector("same"),
                    weight: Some(0),
                }]),
            },
        }])
        .unwrap();
        assert_eq!(
            all_zero
                .decide("virtual", &[], &[backend("same", 0, 3), backend("other", 1, 5)])
                .unwrap()
                .weights,
            vec![0, 0]
        );
    }
}
