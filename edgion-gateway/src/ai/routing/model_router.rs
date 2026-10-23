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
pub struct ModelRouteDecision<'a> {
    pub matched: bool,
    pub identity_model: &'a str,
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

/// Does the request carry every header a route requires?
///
/// Takes the borrowed `HeaderMap` rather than an owned `Vec<(String, String)>`
/// so the AI request path does not copy every header just to test a few of
/// them; lookup is also a hash probe instead of a linear scan. Duplicate field
/// lines are still considered, and a value that is not valid UTF-8 can never
/// equal a configured (UTF-8) expectation, matching the previous behavior.
fn route_headers_match(request_headers: &http::HeaderMap, required: &[(String, String)]) -> bool {
    required.iter().all(|(name, expected)| {
        request_headers
            .get_all(name.as_str())
            .iter()
            .any(|actual| actual.as_bytes() == expected.as_bytes())
    })
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

    pub fn decide<'a>(
        &'a self,
        requested_model: &'a str,
        request_headers: &http::HeaderMap,
        backends: &[CompiledBackend],
    ) -> Result<ModelRouteDecision<'a>, ModelRouteDecisionError> {
        let winner = self
            .routes
            .iter()
            .filter(|route| {
                route.pattern.matches(requested_model) && route_headers_match(request_headers, &route.headers)
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
                identity_model: requested_model,
                weights: backends
                    .iter()
                    .map(|backend| backend.schema().weight.unwrap_or(1))
                    .collect(),
                rewrite_requires_canonical: false,
            });
        };

        let mut weights: Vec<i32> = backends
            .iter()
            .map(|backend| backend.schema().weight.unwrap_or(1))
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
                    weights[backend.slot()] = match overrides.get(&backend.slot()) {
                        Some(Some(weight)) if *weight >= 0 => *weight,
                        Some(Some(_)) => return Err(ModelRouteDecisionError::InvalidWeight),
                        Some(None) => backend.schema().weight.unwrap_or(1),
                        None => 0,
                    };
                }
            }
        }
        Ok(ModelRouteDecision {
            matched: true,
            identity_model: winner.action.rewrite.as_deref().unwrap_or(requested_model),
            weights,
            rewrite_requires_canonical: winner.action.rewrite.is_some(),
        })
    }

    pub fn header_applicable_patterns<'a>(
        &'a self,
        request_headers: &'a http::HeaderMap,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.routes.iter().filter_map(move |route| {
            route_headers_match(request_headers, &route.headers).then_some(route.source_pattern.as_str())
        })
    }

    /// Conservative family-listing guard: a wildcard family is omitted when a
    /// different header-applicable route overlaps any part of its prefix
    /// language. Listing must not claim that the complete family is reachable.
    pub fn wildcard_is_partially_shadowed(&self, wildcard: &str, request_headers: &http::HeaderMap) -> bool {
        let Some(prefix) = wildcard.strip_suffix('*') else {
            return false;
        };
        let header_applicable = |route: &&CompiledModelRoute| route_headers_match(request_headers, &route.headers);
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
        .filter(|backend| backend.identity().matches_ai_selector(selector));
    let slot = matching.next()?.slot();
    matching.next().is_none().then_some(slot)
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/routing/model_router_test.rs"]
mod tests;
