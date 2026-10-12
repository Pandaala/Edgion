//! Immutable, request-readable output of one complete publication transaction.

use crate::routes::grpc::{DomainGrpcRouteRules, GrpcPortRouteManager};
use crate::routes::http::routes_mgr::{DomainRouteRules, HttpPortRouteManager};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Weak};

use edgion_resources::{EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, HTTPRoute, ListenerKey};

use crate::plugins::http::CompiledPlugins;
use crate::plugins::stream::CompiledStreamPlugins;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;

/// One listener's routing and policy generation, retained before early plugins.
/// Tables and scopes retain no publisher or manager ownership. Weak manager
/// identities preserve each physical listener handle's reachability boundary.
pub(crate) struct HttpListenerPublication {
    pub(super) http: Arc<DomainRouteRules>,
    pub(super) grpc: Arc<DomainGrpcRouteRules>,
    pub(super) http_manager: Weak<HttpPortRouteManager>,
    pub(super) grpc_manager: Weak<GrpcPortRouteManager>,
    pub index: Arc<GatewayInfoIndex>,
    owners: ListenerOwners,
    pub(crate) cache_epoch: crate::response_cache::PublicationEpoch,
}

enum ListenerOwners {
    Empty,
    Unique(Arc<GatewayScope>),
    // Children always have Unique owners and no link to this map or siblings.
    Shared(HashMap<String, Arc<HttpListenerPublication>>),
}

impl HttpListenerPublication {
    pub fn empty() -> Arc<Self> {
        static EMPTY: LazyLock<Arc<HttpListenerPublication>> = LazyLock::new(|| {
            Arc::new(HttpListenerPublication {
                http_manager: Weak::new(),
                grpc_manager: Weak::new(),
                http: Arc::new(DomainRouteRules::new()),
                grpc: Arc::new(DomainGrpcRouteRules::new()),
                index: Arc::new(GatewayInfoIndex::from_entries(Vec::new())),
                owners: ListenerOwners::Empty,
                cache_epoch: crate::response_cache::PublicationEpoch::default(),
            })
        });
        Arc::clone(&EMPTY)
    }

    /// Cold construction; shared listeners have exactly one level of owner views.
    pub(super) fn new(
        (http_manager, http): (Weak<HttpPortRouteManager>, Arc<DomainRouteRules>),
        (grpc_manager, grpc): (Weak<GrpcPortRouteManager>, Arc<DomainGrpcRouteRules>),
        index: Arc<GatewayInfoIndex>,
        gateways: HashMap<String, Arc<GatewayScope>>,
    ) -> Arc<Self> {
        let owners = match gateways.len() {
            0 => ListenerOwners::Empty,
            1 => ListenerOwners::Unique(gateways.into_values().next().expect("one gateway")),
            _ => ListenerOwners::Shared(
                gateways
                    .into_iter()
                    .map(|(key, scope)| {
                        let child = Arc::new(Self {
                            http_manager: http_manager.clone(),
                            grpc_manager: grpc_manager.clone(),
                            http: Arc::clone(&http),
                            grpc: Arc::clone(&grpc),
                            index: Arc::clone(&index),
                            owners: ListenerOwners::Unique(scope),
                            cache_epoch: crate::response_cache::PublicationEpoch::default(),
                        });
                        (key, child)
                    })
                    .collect(),
            ),
        };
        Arc::new(Self {
            http_manager,
            grpc_manager,
            http,
            grpc,
            index,
            owners,
            cache_epoch: crate::response_cache::PublicationEpoch::default(),
        })
    }

    /// Cold publication stamping shares routing and scope allocations. Children
    /// receive the same complete token before the new snapshot becomes visible.
    fn with_cache_epoch(&self, epoch: &crate::response_cache::PublicationEpoch) -> Arc<Self> {
        let owners = match &self.owners {
            ListenerOwners::Empty => ListenerOwners::Empty,
            ListenerOwners::Unique(scope) => ListenerOwners::Unique(Arc::clone(scope)),
            ListenerOwners::Shared(children) => ListenerOwners::Shared(
                children
                    .iter()
                    .map(|(key, child)| (key.clone(), child.with_cache_epoch(epoch)))
                    .collect(),
            ),
        };
        Arc::new(Self {
            http: Arc::clone(&self.http),
            grpc: Arc::clone(&self.grpc),
            http_manager: self.http_manager.clone(),
            grpc_manager: self.grpc_manager.clone(),
            index: Arc::clone(&self.index),
            owners,
            cache_epoch: epoch.clone(),
        })
    }

    /// Compare stable manager identity, never the manager's mutable table.
    pub fn http_routes(&self, manager: &Arc<HttpPortRouteManager>) -> Arc<DomainRouteRules> {
        if std::ptr::eq(self.http_manager.as_ptr(), Arc::as_ptr(manager)) {
            Arc::clone(&self.http)
        } else {
            Arc::clone(&Self::empty().http)
        }
    }

    pub fn grpc_routes(&self, manager: &Arc<GrpcPortRouteManager>) -> Arc<DomainGrpcRouteRules> {
        if std::ptr::eq(self.grpc_manager.as_ptr(), Arc::as_ptr(manager)) {
            Arc::clone(&self.grpc)
        } else {
            Arc::clone(&Self::empty().grpc)
        }
    }

    pub fn gateway(&self, key: &str) -> Option<Arc<GatewayScope>> {
        match &self.owners {
            ListenerOwners::Empty => None,
            ListenerOwners::Unique(scope) => (scope.gateway_key == key).then(|| Arc::clone(scope)),
            ListenerOwners::Shared(owners) => owners.get(key).and_then(|view| view.unique_gateway()).cloned(),
        }
    }

    pub fn unique_gateway(&self) -> Option<&Arc<GatewayScope>> {
        match &self.owners {
            ListenerOwners::Unique(scope) => Some(scope),
            _ => None,
        }
    }

    pub(super) fn gateway_count(&self) -> usize {
        match &self.owners {
            ListenerOwners::Empty => 0,
            ListenerOwners::Unique(_) => 1,
            ListenerOwners::Shared(owners) => owners.len(),
        }
    }

    pub fn select_gateway(self: &Arc<Self>, key: &str) -> Option<Arc<Self>> {
        match &self.owners {
            ListenerOwners::Empty => None,
            ListenerOwners::Unique(scope) => (scope.gateway_key == key).then(|| Arc::clone(self)),
            ListenerOwners::Shared(owners) => owners.get(key).cloned(),
        }
    }
}

pub(crate) struct GatewayScope {
    pub gateway_key: String,
    pub scope: Arc<PluginScope>,
    pub http_keys: Vec<String>,
}

pub(crate) struct RuleAdmission {
    pub source: Weak<HTTPRoute>,
    pub allowed: Vec<bool>,
    pub graphql_allowed: Vec<bool>,
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
    pub fn graphql_rule(&self, route: &Arc<HTTPRoute>, index: usize) -> bool {
        self.available
            && self.rules.get(&(Arc::as_ptr(route) as usize)).is_some_and(|gate| {
                std::ptr::eq(gate.source.as_ptr(), Arc::as_ptr(route))
                    && gate.graphql_allowed.get(index).copied().unwrap_or(false)
            })
    }

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
    pub(crate) cache_epoch: crate::response_cache::PublicationEpoch,
    pub(super) gateways: HashMap<String, Arc<GatewayScope>>,
    pub(super) http_listeners: HashMap<ListenerKey, Arc<HttpListenerPublication>>,
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
    pub(super) fn stamp_cache_epoch(&mut self, epoch: crate::response_cache::PublicationEpoch) {
        if epoch.id == 0 && !epoch.is_initialized() {
            return;
        }
        for view in self.http_listeners.values_mut() {
            *view = view.with_cache_epoch(&epoch);
        }
        self.cache_epoch = epoch;
    }

    pub fn http_listener(&self, key: &ListenerKey) -> Arc<HttpListenerPublication> {
        self.http_listeners
            .get(key)
            .cloned()
            .unwrap_or_else(HttpListenerPublication::empty)
    }

    pub fn gateway(&self, key: &str) -> Option<Arc<GatewayScope>> {
        self.gateways.get(key).cloned()
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
