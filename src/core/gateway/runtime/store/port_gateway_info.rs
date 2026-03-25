//! Port-based GatewayInfo Store
//!
//! Maintains a dynamic mapping from listener port to all Gateway/Listener
//! contexts sharing that port. This enables correct route matching when
//! Gateways are added or removed at runtime, without requiring a restart.
//!
//! ## Why This Exists
//!
//! Route matching via `deep_match` → `check_gateway_listener_match` needs the
//! list of GatewayInfo for the request's port.  Previously this list was baked
//! into the HTTP proxy listener at startup and never updated. If a new Gateway was added
//! at runtime sharing an already-bound port, its routes would silently fail to
//! match.  This store is rebuilt on every Gateway change, keeping the list
//! current.
//!
//! ## Performance
//!
//! Uses `ArcSwap` for lock-free reads on the hot path. Rebuilds (writes) happen
//! only on Gateway create/update/delete, which is rare.

use super::super::gateway_info::GatewayInfo;
use crate::core::gateway::observe::metrics::TestType;
use crate::types::constants::annotations::edgion as annotations;
use crate::types::Gateway;
use arc_swap::ArcSwap;
use kube::ResourceExt;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

pub struct PortGatewayInfoStore {
    flat_data: ArcSwap<HashMap<u16, Arc<Vec<GatewayInfo>>>>,
    grouped_data: ArcSwap<HashMap<u16, Arc<HashMap<String, Arc<Vec<GatewayInfo>>>>>>,
}

impl PortGatewayInfoStore {
    pub fn new() -> Self {
        Self {
            flat_data: ArcSwap::from_pointee(HashMap::new()),
            grouped_data: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Get all GatewayInfo contexts for a given port.
    ///
    /// Returns an empty Vec (behind Arc) if the port has no registered Gateways.
    /// This is the hot-path call used in `pg_request_filter` per request.
    #[inline]
    pub fn get(&self, port: u16) -> Arc<Vec<GatewayInfo>> {
        let data = self.flat_data.load();
        data.get(&port).cloned().unwrap_or_else(|| Arc::new(Vec::new()))
    }

    /// Return all ports that have at least one registered Gateway/Listener.
    pub fn all_ports(&self) -> Vec<u16> {
        let data = self.flat_data.load();
        data.keys().copied().collect()
    }

    /// Get GatewayInfo contexts grouped by Gateway key for a given port.
    #[inline]
    pub fn get_grouped(&self, port: u16) -> Arc<HashMap<String, Arc<Vec<GatewayInfo>>>> {
        let data = self.grouped_data.load();
        data.get(&port).cloned().unwrap_or_else(|| Arc::new(HashMap::new()))
    }

    /// Rebuild the store from the full list of Gateway resources.
    ///
    /// Iterates all Gateways and their Listeners, skipping conflicted ones,
    /// and builds a port → Vec<GatewayInfo> mapping.
    pub fn rebuild(&self, gateways: &[Gateway]) {
        let mut port_map: HashMap<u16, Vec<GatewayInfo>> = HashMap::new();
        let mut grouped_port_map: HashMap<u16, HashMap<String, Vec<GatewayInfo>>> = HashMap::new();

        for gateway in gateways {
            let gateway_namespace = gateway.metadata.namespace.clone();
            let gateway_name = gateway.name_any();

            let listeners = match &gateway.spec.listeners {
                Some(l) => l,
                None => continue,
            };

            let gateway_annotations_map: HashMap<String, String> = gateway
                .metadata
                .annotations
                .clone()
                .map(|btree| btree.into_iter().collect())
                .unwrap_or_default();

            let metrics_test_key = gateway_annotations_map.get(annotations::METRICS_TEST_KEY).cloned();
            let metrics_test_type = gateway_annotations_map
                .get(annotations::METRICS_TEST_TYPE)
                .map(|s| TestType::from_str(s));

            for listener in listeners {
                if is_listener_conflicted(gateway, &listener.name) {
                    continue;
                }

                let port = listener.port as u16;
                let gi = GatewayInfo::new(
                    gateway_namespace.clone(),
                    gateway_name.clone(),
                    Some(listener.name.clone()),
                    metrics_test_key.clone(),
                    metrics_test_type.clone(),
                );
                port_map.entry(port).or_default().push(gi.clone());
                grouped_port_map
                    .entry(port)
                    .or_default()
                    .entry(gi.gateway_key())
                    .or_default()
                    .push(gi);
            }
        }

        let port_count = port_map.len();
        let total_entries: usize = port_map.values().map(|v| v.len()).sum();

        let flat_arc_map: HashMap<u16, Arc<Vec<GatewayInfo>>> = port_map
            .into_iter()
            .map(|(port, infos)| (port, Arc::new(infos)))
            .collect();

        let grouped_arc_map: HashMap<u16, Arc<HashMap<String, Arc<Vec<GatewayInfo>>>>> = grouped_port_map
            .into_iter()
            .map(|(port, gateway_infos)| {
                let per_gateway = gateway_infos
                    .into_iter()
                    .map(|(gateway_key, infos)| (gateway_key, Arc::new(infos)))
                    .collect();
                (port, Arc::new(per_gateway))
            })
            .collect();

        self.flat_data.store(Arc::new(flat_arc_map));
        self.grouped_data.store(Arc::new(grouped_arc_map));

        tracing::info!(
            component = "port_gateway_info_store",
            ports = port_count,
            gateway_listener_entries = total_entries,
            "Rebuilt port GatewayInfo store"
        );
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PortGatewayInfoStats {
    pub port_count: usize,
    pub total_entries: usize,
}

impl PortGatewayInfoStore {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> PortGatewayInfoStats {
        let data = self.flat_data.load();
        let total: usize = data.values().map(|v| v.len()).sum();
        PortGatewayInfoStats {
            port_count: data.len(),
            total_entries: total,
        }
    }
}

impl Default for PortGatewayInfoStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a Listener is marked as Conflicted in Gateway status.
///
/// Returns true if the Listener has a Conflicted condition with status "True".
/// This is set by the Controller when port conflicts are detected.
fn is_listener_conflicted(gateway: &Gateway, listener_name: &str) -> bool {
    gateway
        .status
        .as_ref()
        .and_then(|s| s.listeners.as_ref())
        .and_then(|listeners| listeners.iter().find(|ls| ls.name == listener_name))
        .map(|ls| {
            ls.conditions
                .iter()
                .any(|c| c.type_ == "Conflicted" && c.status == "True")
        })
        .unwrap_or(false)
}

static GLOBAL_PORT_GATEWAY_INFO_STORE: LazyLock<PortGatewayInfoStore> = LazyLock::new(PortGatewayInfoStore::new);

/// Get a reference to the global PortGatewayInfoStore
pub fn get_port_gateway_info_store() -> &'static PortGatewayInfoStore {
    &GLOBAL_PORT_GATEWAY_INFO_STORE
}

/// Rebuild the global PortGatewayInfoStore from Gateway list
pub fn rebuild_port_gateway_infos(gateways: &[Gateway]) {
    get_port_gateway_info_store().rebuild(gateways);
}
