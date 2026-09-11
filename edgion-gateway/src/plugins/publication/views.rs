//! Immutable, request-readable output of one complete publication transaction.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use edgion_resources::{EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, HTTPRoute, ListenerKey};

use crate::plugins::http::CompiledPlugins;
use crate::plugins::stream::CompiledStreamPlugins;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;

pub(crate) struct GatewayScope {
    pub gateway_key: String,
    pub scope: Arc<PluginScope>,
    pub http_keys: Vec<String>,
}

pub(crate) struct RuleAdmission {
    pub source: Weak<HTTPRoute>,
    pub allowed: Vec<bool>,
}

pub(crate) struct PluginScope {
    pub available: bool,
    pub config: Option<Arc<EdgionGatewayConfig>>,
    pub global_keys: Vec<String>,
    pub has_access_log_extern: bool,
    pub(super) http: HashMap<String, Arc<CompiledPlugins>>,
    pub(super) stream: HashMap<String, Arc<CompiledStreamPlugins>>,
    pub(super) rules: HashMap<usize, RuleAdmission>,
}

impl PluginScope {
    pub fn http(&self, key: &str) -> Option<Arc<CompiledPlugins>> {
        self.http.get(key).cloned()
    }

    pub fn stream(&self, key: &str) -> Option<Arc<CompiledStreamPlugins>> {
        self.stream.get(key).cloned()
    }

    /// The Weak token prevents allocation reuse without retaining the route.
    /// A route table from another publication must never borrow this decision.
    pub fn ai_rule(&self, route: &Arc<HTTPRoute>, index: usize) -> bool {
        self.available
            && self.rules.get(&(Arc::as_ptr(route) as usize)).is_some_and(|gate| {
                std::ptr::eq(gate.source.as_ptr(), Arc::as_ptr(route))
                    && gate.allowed.get(index).copied().unwrap_or(false)
            })
    }
}

#[derive(Clone)]
pub(crate) enum ConnectionView {
    Empty,
    Unavailable,
    Ready(Arc<CompiledStreamPlugins>),
}

#[derive(Clone, Copy, Default, serde::Serialize)]
pub(crate) struct PublicationStats {
    pub scopes: usize,
    pub http_generations: usize,
    pub http_cached_entries: usize,
    pub stream_cached_entries: usize,
    pub ai_route_tokens: usize,
    pub listeners: usize,
}

#[derive(Clone, Default, serde::Serialize)]
pub(crate) struct PublicationInventory {
    pub scopes: Vec<String>,
    pub http_generations: Vec<String>,
    pub http_entries: Vec<String>,
    pub stream_entries: Vec<String>,
    pub ai_routes: Vec<String>,
}

#[derive(Default)]
pub(crate) struct PluginPublication {
    pub(super) gateways: HashMap<String, Arc<GatewayScope>>,
    pub(super) scopes: HashMap<String, Arc<PluginScope>>,
    pub(super) listeners: HashMap<ListenerKey, Vec<String>>,
    pub(super) indices: HashMap<ListenerKey, Arc<GatewayInfoIndex>>,
    pub(super) index_signatures: HashMap<ListenerKey, Vec<(String, u64)>>,
    pub(super) connections: HashMap<ListenerKey, ConnectionView>,
    pub(super) http_sources: HashMap<String, Arc<EdgionPlugins>>,
    pub(super) stream_sources: HashMap<String, Arc<EdgionStreamPlugins>>,
    pub(super) stats: PublicationStats,
    pub(super) inventory: PublicationInventory,
}

impl PluginPublication {
    pub fn gateway(&self, key: &str) -> Option<Arc<GatewayScope>> {
        self.gateways.get(key).cloned()
    }

    pub fn unique_gateway(&self, key: &ListenerKey) -> Option<Arc<GatewayScope>> {
        match self.listeners.get(key)?.as_slice() {
            [key] => self.gateway(key),
            _ => None,
        }
    }

    pub fn connection(&self, key: &ListenerKey) -> ConnectionView {
        self.connections
            .get(key)
            .cloned()
            .unwrap_or(ConnectionView::Unavailable)
    }

    pub fn listener_index(&self, key: &ListenerKey) -> Option<&Arc<GatewayInfoIndex>> {
        self.indices.get(key)
    }

    pub fn http_sources(&self) -> &HashMap<String, Arc<EdgionPlugins>> {
        &self.http_sources
    }

    pub fn stream_sources(&self) -> &HashMap<String, Arc<EdgionStreamPlugins>> {
        &self.stream_sources
    }

    /// Configuration-only capability lookup. Requests use their captured scope.
    pub fn any_http(&self, key: &str, predicate: impl Fn(&CompiledPlugins) -> bool) -> bool {
        self.scopes
            .values()
            .filter(|scope| scope.available)
            .filter_map(|scope| scope.http.get(key))
            .any(|view| predicate(view))
    }

    pub fn stats(&self) -> PublicationStats {
        self.stats
    }

    /// Sorted debug identities for the existing cache-residue inspection path.
    pub fn inventory(&self) -> &PublicationInventory {
        &self.inventory
    }
}
