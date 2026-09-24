//! POST /api/v1/admin/wipe-all — testing-mode only. Deletes every resource
//! from ConfCenter in tier order so downstream caches receive delete events
//! through the normal watch path.
//!
//! Tier 1 (consumers): HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute,
//!                     EdgionPlugins, EdgionStreamPlugins, BackendTLSPolicy,
//!                     EdgionBackendTrafficPolicy
//! Tier 2 (config):    Gateway, GatewayClass, EdgionGatewayConfig, EdgionTls, EdgionAcme,
//!                     EdgionConfigData, LinkSys, EdgionBackend, ReferenceGrant
//! Tier 3 (low-level): Service, EndpointSlice, Secret, ConfigMap

use crate::api::types::AdminState;
use crate::api::ApiResponse;
use axum::{extract::State, response::Json};
use edgion_resources::ResourceKind;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Serialize)]
pub struct WipeAllResponse {
    pub deleted: BTreeMap<String, Vec<String>>,
    pub total: usize,
    pub errors: Vec<WipeError>,
    pub duration_ms: u128,
}

#[derive(Serialize)]
pub struct WipeError {
    pub kind: String,
    pub name: String,
    pub reason: String,
}

const TIER_1: &[ResourceKind] = &[
    ResourceKind::HTTPRoute,
    ResourceKind::GRPCRoute,
    ResourceKind::TCPRoute,
    ResourceKind::UDPRoute,
    ResourceKind::TLSRoute,
    ResourceKind::EdgionPlugins,
    ResourceKind::EdgionStreamPlugins,
    ResourceKind::BackendTLSPolicy,
    ResourceKind::EdgionBackendTrafficPolicy,
];

const TIER_2: &[ResourceKind] = &[
    ResourceKind::Gateway,
    ResourceKind::GatewayClass,
    ResourceKind::EdgionGatewayConfig,
    ResourceKind::EdgionTls,
    ResourceKind::EdgionAcme,
    ResourceKind::EdgionConfigData,
    ResourceKind::LinkSys,
    ResourceKind::EdgionBackend,
    ResourceKind::ReferenceGrant,
];

const TIER_3: &[ResourceKind] = &[
    ResourceKind::Service,
    ResourceKind::EndpointSlice,
    ResourceKind::Secret,
    ResourceKind::ConfigMap,
];

pub async fn wipe_all(State(state): State<Arc<AdminState>>) -> Json<ApiResponse<WipeAllResponse>> {
    let start = Instant::now();
    tracing::info!(component = "wipe_all", "wipe_all started");
    let mut deleted: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut errors: Vec<WipeError> = Vec::new();

    for tier in [TIER_1, TIER_2, TIER_3] {
        wipe_tier(&state, tier, &mut deleted, &mut errors).await;
    }

    let total: usize = deleted.values().map(|v| v.len()).sum();
    let duration_ms = Instant::now().saturating_duration_since(start).as_millis();
    tracing::info!(
        component = "wipe_all",
        total = total,
        errors = errors.len(),
        duration_ms = duration_ms as u64,
        "wipe_all finished"
    );
    Json(ApiResponse::ok_body(WipeAllResponse {
        deleted,
        total,
        errors,
        duration_ms,
    }))
}

async fn wipe_tier(
    state: &AdminState,
    kinds: &[ResourceKind],
    deleted: &mut BTreeMap<String, Vec<String>>,
    errors: &mut Vec<WipeError>,
) {
    let futures: Vec<_> = kinds.iter().map(|k| wipe_kind(state, *k)).collect();
    let results = futures::future::join_all(futures).await;
    for (kind, (kind_deleted, kind_errors)) in kinds.iter().zip(results) {
        deleted
            .entry(kind.as_str().to_string())
            .or_default()
            .extend(kind_deleted);
        errors.extend(kind_errors);
    }
}

async fn wipe_kind(state: &AdminState, kind: ResourceKind) -> (Vec<String>, Vec<WipeError>) {
    let writer = state.conf_center();
    let mut deleted = Vec::new();
    let mut errors = Vec::new();

    // The list is winner-only (one entry per (kind,key)). Deleting a winner
    // promotes any conflict loser to winner (PathRegistry::mark_api_deleted), so
    // a re-list surfaces it next pass. Loop until the kind is empty. Bounded by
    // the maximum conflict depth for a key; the no-progress guard plus the hard
    // cap prevent an infinite loop if a delete persistently fails.
    const MAX_PASSES: usize = 100;
    for _ in 0..MAX_PASSES {
        let items = match writer.get_list_by_kind(kind.as_str(), None).await {
            Ok(list) => list.items,
            Err(e) => {
                errors.push(WipeError {
                    kind: kind.as_str().to_string(),
                    name: String::new(),
                    reason: format!("list failed: {}", e),
                });
                break;
            }
        };
        if items.is_empty() {
            break;
        }

        let mut deleted_this_pass = 0usize;
        for item in items {
            let (ns, name) = (item.namespace.clone(), item.name.clone());
            let key = match &ns {
                Some(n) => format!("{}/{}", n, name),
                None => name.clone(),
            };
            match writer.delete_one(kind.as_str(), ns.as_deref(), &name).await {
                Ok(_) => {
                    deleted.push(key);
                    deleted_this_pass += 1;
                }
                Err(e) => errors.push(WipeError {
                    kind: kind.as_str().to_string(),
                    name: key,
                    reason: format!("{}", e),
                }),
            }
        }

        // No successful delete this pass -> remaining items are stuck; stop
        // rather than spin (the hard cap is a secondary backstop).
        if deleted_this_pass == 0 {
            break;
        }
    }

    (deleted, errors)
}

#[cfg(test)]
#[path = "../../tests/unit/api/wipe_handlers_test.rs"]
mod tests;
