//! Serialized cold-path inputs and atomic policy/executable publication.

use edgion_resources::resources::refs::gateway_class_fields::GatewayClassParametersRef;
use edgion_resources::resources::refs::plugin_attachment_fields::HttpPluginAttachments;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use serde::Serialize;

use crate::plugins::http::CompiledPlugins;
use crate::plugins::runtime::gateway_api_filters::extension_ref::extension_ref_key;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::http_route_ai_admission::{ai_rule_invalid_reason, AiProxyAdmission};
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily, PluginPolicyAction};
use edgion_resources::resources::refs::plugin_target::{classify, PluginTarget};
use edgion_resources::resources::refs::route_fields::HttpRouteExtensionRefs;

use edgion_resources::{EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, GatewayClass, HTTPRoute, ListenerKey};

use super::inputs::{effective_http_rules, GatewayInput, RouteInput, RouteKind};
use super::views::{ConnectionView, GatewayScope, PluginPublication, PluginScope, PublicationStats, RuleAdmission};
use crate::plugins::http::plugin_generation::PluginGeneration;
use crate::plugins::runtime::entry_bundle::EntryDemand;
use crate::plugins::stream::plugin_generation::{StreamEntryDemand, StreamPluginGeneration, StreamStages};
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};

static PUBLISHER: LazyLock<PluginPublisher> = LazyLock::new(PluginPublisher::new);

pub(crate) fn global_publisher() -> &'static PluginPublisher {
    &PUBLISHER
}

pub(crate) struct PluginPublisher {
    state: Mutex<Inputs>,
    publication: ArcSwap<PluginPublication>,
}

#[derive(Clone)]
struct Selection {
    raw_key: String,
    latest: u64,
    executable: u64,
    permitted: EntryDemand,
    activatable: bool,
}

#[derive(Default)]
struct Inputs {
    gateway_configs: HashMap<String, Arc<EdgionGatewayConfig>>,
    gateway_classes: HashMap<String, Arc<GatewayClass>>,
    gateways: HashMap<String, GatewayInput>,
    gateway_versions: HashMap<String, u64>,
    routes: HashMap<(RouteKind, String), RouteInput>,
    http_sources: HashMap<String, Arc<EdgionPlugins>>,
    stream_sources: HashMap<String, Arc<EdgionStreamPlugins>>,
    http_latest: HashMap<String, u64>,
    http_generations: HashMap<u64, PluginGeneration>,
    stream_ids: HashMap<String, u64>,
    stream_generations: HashMap<String, StreamPluginGeneration>,
    selected: HashMap<String, HashMap<String, Selection>>,
    signatures: HashMap<String, ScopeSignature>,
    next_id: u64,
}

#[derive(PartialEq, Eq)]
struct ScopeSignature {
    available: bool,
    config: usize,
    policy: CompiledPluginPolicy,
    globals: Vec<String>,
    http: HashMap<String, PluginTarget<u64>>,
    stream: HashMap<String, (Option<u64>, StreamStages)>,
    routes: Vec<usize>,
}

struct ScopePlan {
    signature: ScopeSignature,
    config: Option<Arc<EdgionGatewayConfig>>,
    routes: Vec<Arc<HTTPRoute>>,
}

pub(crate) fn same_source<T: Serialize>(left: &Arc<T>, right: &Arc<T>) -> bool {
    Arc::ptr_eq(left, right)
        || matches!((serde_json::to_value(left.as_ref()), serde_json::to_value(right.as_ref())), (Ok(a), Ok(b)) if a == b)
}

fn replace_sources<T: Serialize>(current: &mut HashMap<String, Arc<T>>, mut incoming: HashMap<String, Arc<T>>) {
    for (key, source) in &mut incoming {
        if let Some(old) = current.get(key).filter(|old| same_source(old, source)) {
            *source = Arc::clone(old);
        }
    }
    *current = incoming;
}

fn update_sources<T: Serialize>(
    current: &mut HashMap<String, Arc<T>>,
    incoming: HashMap<String, Arc<T>>,
    removed: &HashSet<String>,
) {
    current.retain(|key, _| !removed.contains(key));
    for (key, source) in incoming {
        if current.get(&key).is_none_or(|old| !same_source(old, &source)) {
            current.insert(key, source);
        }
    }
}

macro_rules! source_callbacks {
    ($replace:ident, $update:ident, $field:ident, $ty:ty) => {
        pub fn $replace(&self, incoming: HashMap<String, Arc<$ty>>) {
            self.change(|state| replace_sources(&mut state.$field, incoming));
        }
        pub fn $update(&self, incoming: HashMap<String, Arc<$ty>>, removed: &HashSet<String>) {
            self.change(|state| update_sources(&mut state.$field, incoming, removed));
        }
    };
}

impl Default for PluginPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginPublisher {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(Inputs::default()),
            publication: ArcSwap::from_pointee(PluginPublication::default()),
        }
    }

    pub fn snapshot(&self) -> Arc<PluginPublication> {
        self.publication.load_full()
    }

    /// Callers pass owned facts after releasing their own rebuild/store locks.
    /// Nothing below reads ClientCache, a route manager or another runtime store.
    fn change(&self, edit: impl FnOnce(&mut Inputs)) {
        let mut state = self.state.lock();
        edit(&mut state);
        let previous = self.publication.load_full();
        let next = state.compose(&previous);
        self.publication.store(Arc::new(next));
    }

    source_callbacks!(
        replace_gateway_configs,
        update_gateway_configs,
        gateway_configs,
        EdgionGatewayConfig
    );
    source_callbacks!(
        replace_gateway_classes,
        update_gateway_classes,
        gateway_classes,
        GatewayClass
    );

    pub fn replace_gateways(&self, incoming: HashMap<String, GatewayInput>) {
        self.change(|state| {
            state.gateways.retain(|key, _| incoming.contains_key(key));
            state.gateway_versions.retain(|key, _| incoming.contains_key(key));
            state.update_gateways(incoming);
        });
    }

    pub fn update_gateways(&self, incoming: HashMap<String, GatewayInput>, removed: &HashSet<String>) {
        self.change(|state| {
            state.gateways.retain(|key, _| !removed.contains(key));
            state.gateway_versions.retain(|key, _| !removed.contains(key));
            state.update_gateways(incoming);
        });
    }

    pub fn replace_routes(&self, kind: RouteKind, incoming: HashMap<String, RouteInput>) {
        self.change(|state| {
            state.routes.retain(|(current_kind, _), _| *current_kind != kind);
            state
                .routes
                .extend(incoming.into_iter().map(|(key, route)| ((kind, key), route)));
        });
    }

    pub fn update_routes(&self, kind: RouteKind, incoming: HashMap<String, RouteInput>, removed: &HashSet<String>) {
        self.change(|state| {
            state
                .routes
                .retain(|(current_kind, key), _| *current_kind != kind || !removed.contains(key));
            state
                .routes
                .extend(incoming.into_iter().map(|(key, route)| ((kind, key), route)));
        });
    }

    pub fn replace_http_plugins(&self, incoming: HashMap<String, Arc<EdgionPlugins>>) {
        self.change(|state| {
            let removed = state
                .http_sources
                .keys()
                .filter(|key| !incoming.contains_key(*key))
                .cloned()
                .collect();
            state.remove_http(&removed);
            for (key, source) in incoming {
                state.set_http(key, source, false);
            }
        });
    }

    /// An add, including a same-key add, starts a fresh retention boundary.
    pub fn update_http_plugins(
        &self,
        added: HashMap<String, Arc<EdgionPlugins>>,
        updated: HashMap<String, Arc<EdgionPlugins>>,
        removed: &HashSet<String>,
    ) {
        self.change(|state| {
            state.remove_http(removed);
            for (key, source) in updated {
                state.set_http(key, source, false);
            }
            for (key, source) in added {
                state.set_http(key, source, true);
            }
        });
    }

    pub fn replace_stream_plugins(&self, incoming: HashMap<String, Arc<EdgionStreamPlugins>>) {
        self.change(|state| {
            state.stream_sources.retain(|key, _| incoming.contains_key(key));
            state.stream_ids.retain(|key, _| incoming.contains_key(key));
            state.stream_generations.retain(|key, _| incoming.contains_key(key));
            state.set_stream(incoming);
        });
    }

    pub fn update_stream_plugins(
        &self,
        incoming: HashMap<String, Arc<EdgionStreamPlugins>>,
        removed: &HashSet<String>,
    ) {
        self.change(|state| {
            state.stream_sources.retain(|key, _| !removed.contains(key));
            state.stream_ids.retain(|key, _| !removed.contains(key));
            state.stream_generations.retain(|key, _| !removed.contains(key));
            state.set_stream(incoming);
        });
    }
}

impl Inputs {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn remove_http(&mut self, removed: &HashSet<String>) {
        self.http_sources.retain(|key, _| !removed.contains(key));
        self.http_latest.retain(|key, _| !removed.contains(key));
        for selected in self.selected.values_mut() {
            selected.retain(|_, selection| !removed.contains(&selection.raw_key));
        }
    }

    fn set_http(&mut self, key: String, source: Arc<EdgionPlugins>, fresh: bool) {
        if !fresh && self.http_sources.get(&key).is_some_and(|old| same_source(old, &source)) {
            return;
        }
        if fresh {
            for selected in self.selected.values_mut() {
                selected.retain(|_, selection| selection.raw_key != key);
            }
        }
        let id = self.id();
        self.http_generations.insert(id, PluginGeneration::from_source(&source));
        self.http_latest.insert(key.clone(), id);
        self.http_sources.insert(key, source);
    }

    fn set_stream(&mut self, incoming: HashMap<String, Arc<EdgionStreamPlugins>>) {
        for (key, source) in incoming {
            if self
                .stream_sources
                .get(&key)
                .is_some_and(|old| same_source(old, &source))
            {
                continue;
            }
            let id = self.id();
            self.stream_ids.insert(key.clone(), id);
            self.stream_generations
                .insert(key.clone(), StreamPluginGeneration::new(Arc::clone(&source)));
            self.stream_sources.insert(key, source);
        }
    }

    fn update_gateways(&mut self, incoming: HashMap<String, GatewayInput>) {
        for (key, gateway) in incoming {
            let unchanged = self.gateways.get(&key).is_some_and(|old| {
                old.class_name == gateway.class_name
                    && old.http_keys == gateway.http_keys
                    && old.connection_key == gateway.connection_key
                    && old.listeners.len() == gateway.listeners.len()
                    && old.listeners.iter().zip(&gateway.listeners).all(|(a, b)| {
                        a.key == b.key
                            && a.name == b.name
                            && a.hostname == b.hostname
                            && serde_json::to_vec(&a.allowed_routes).ok() == serde_json::to_vec(&b.allowed_routes).ok()
                    })
            });
            if !unchanged {
                let id = self.id();
                self.gateway_versions.insert(key.clone(), id);
                self.gateways.insert(key, gateway);
            }
        }
    }

    fn scope_plan(&self, class: &str, gateways: &[(&String, &GatewayInput)]) -> ScopePlan {
        let mut available = true;
        let config = match self.gateway_classes.get(class) {
            None => {
                available = false;
                None
            }
            Some(class) => match GatewayClassParametersRef::from_class(class) {
                Ok(None) => None,
                Ok(Some(reference)) => {
                    let config = self.gateway_configs.get(reference.target().name()).cloned();
                    available = config.is_some();
                    config
                }
                Err(_) => {
                    available = false;
                    None
                }
            },
        };
        let policy = match config
            .as_ref()
            .and_then(|config| config.spec.plugin_policy.as_ref())
            .map(|policy| policy.compile())
        {
            Some(Ok(policy)) => policy,
            Some(Err(_)) => {
                available = false;
                CompiledPluginPolicy::default()
            }
            None => CompiledPluginPolicy::default(),
        };
        let globals: Vec<String> = config
            .as_ref()
            .into_iter()
            .flat_map(|config| HttpPluginAttachments::EgcGlobals(config).entries())
            .flatten()
            .map(|reference| reference.into_key())
            .collect();
        let mut http: HashSet<String> = globals.iter().cloned().collect();
        let mut stream: HashMap<String, StreamStages> = HashMap::new();
        for (_, gateway) in gateways {
            http.extend(gateway.http_keys.iter().cloned());
            if let Some(key) = gateway.connection_key.as_ref().filter(|key| !key.is_empty()) {
                stream.entry(key.clone()).or_default().connection = true;
            }
        }
        let mut routes = Vec::new();
        for route in self.routes.values() {
            if !gateways.iter().any(|(key, gateway)| route.attaches_to(key, gateway)) {
                continue;
            }
            http.extend(route.http_keys.iter().cloned());
            if let Some(key) = route.stream_key.as_ref().filter(|key| !key.is_empty()) {
                stream.entry(key.clone()).or_default().extend(route.stages);
            }
            if let Some(route) = &route.http_route {
                routes.push(Arc::clone(route));
            }
        }
        routes.sort_by_key(|route| Arc::as_ptr(route) as usize);
        ScopePlan {
            signature: ScopeSignature {
                available,
                config: config.as_ref().map_or(0, |config| Arc::as_ptr(config) as usize),
                policy,
                globals,
                http: http
                    .into_iter()
                    .map(|key| {
                        let id = classify(&key, |raw_key| self.http_latest.get(raw_key).copied())
                            .unwrap_or(PluginTarget::Missing);
                        (key, id)
                    })
                    .collect(),
                stream: stream
                    .into_iter()
                    .map(|(key, stages)| {
                        let id = self.stream_ids.get(&key).copied();
                        (key, (id, stages))
                    })
                    .collect(),
                routes: routes.iter().map(|route| Arc::as_ptr(route) as usize).collect(),
            },
            config,
            routes,
        }
    }

    fn compose(&mut self, previous: &PluginPublication) -> PluginPublication {
        let mut classes: HashMap<&str, Vec<(&String, &GatewayInput)>> = HashMap::new();
        for (key, gateway) in &self.gateways {
            if !gateway.listeners.is_empty() {
                classes.entry(&gateway.class_name).or_default().push((key, gateway));
            }
        }
        let plans: HashMap<String, ScopePlan> = classes
            .into_iter()
            .map(|(class, gateways)| (class.to_owned(), self.scope_plan(class, &gateways)))
            .collect();
        let mut next = PluginPublication {
            http_sources: self.http_sources.clone(),
            stream_sources: self.stream_sources.clone(),
            ..Default::default()
        };
        let mut selected = HashMap::new();
        let mut http_demand: HashMap<u64, EntryDemand> = HashMap::new();
        let mut stream_demand: HashMap<String, StreamEntryDemand> = HashMap::new();
        let mut signatures = HashMap::new();
        for (class, plan) in plans {
            let old_signature = self.signatures.get(&class);
            let old_scope = previous.scopes.get(&class);
            let old_selected = self.selected.get(&class);
            let unchanged = old_signature == Some(&plan.signature) && old_scope.is_some();
            let mut selection = HashMap::new();
            let scope = if unchanged {
                selection = old_selected.cloned().unwrap_or_default();
                Arc::clone(old_scope.unwrap())
            } else {
                let mut scope = PluginScope {
                    available: plan.signature.available,
                    config: plan.config,
                    global_keys: plan.signature.globals.clone(),
                    has_access_log_extern: false,
                    http: HashMap::new(),
                    stream: HashMap::new(),
                    rules: HashMap::new(),
                };
                if scope.available {
                    for (key, latest) in &plan.signature.http {
                        let (raw_key, latest) = match latest {
                            PluginTarget::Missing => continue,
                            PluginTarget::Ambiguous => {
                                scope
                                    .http
                                    .insert(key.clone(), Arc::new(CompiledPlugins::ambiguous_reference()));
                                continue;
                            }
                            PluginTarget::Unique { raw_key, source } => (raw_key, *source),
                        };
                        let prior = old_selected
                            .and_then(|selection| selection.get(key))
                            .filter(|selection| selection.raw_key == *raw_key);
                        let same_policy =
                            old_signature.is_some_and(|old| old.available && old.policy == plan.signature.policy);
                        if let Some(prior) = prior.filter(|prior| same_policy && prior.latest == latest) {
                            if let Some(view) = old_scope.and_then(|scope| scope.http.get(key)) {
                                scope.http.insert(key.clone(), Arc::clone(view));
                                selection.insert(key.clone(), prior.clone());
                                continue;
                            }
                        }
                        let generation = self
                            .http_generations
                            .get_mut(&latest)
                            .expect("latest source owns its generation");
                        let incoming = generation.compile(Some(&plan.signature.policy));
                        let extra_actions = generation.policy_actions(Some(&plan.signature.policy));
                        let mut chosen = Arc::clone(&incoming);
                        let mut executable = latest;
                        if !incoming.is_activatable && incoming.retain_previous_on_invalid_update {
                            if let Some(prior) = prior.filter(|prior| prior.activatable && prior.executable != latest) {
                                if let Some(candidate) = self.http_generations.get_mut(&prior.executable) {
                                    let needed = candidate.demand(Some(&plan.signature.policy));
                                    // A relaxation must never reconstruct a slot excluded from
                                    // the previously selected candidate while latest is invalid.
                                    if needed.is_subset(&prior.permitted) {
                                        let retained = candidate.compile_with_additional_actions(
                                            Some(&plan.signature.policy),
                                            &extra_actions,
                                        );
                                        if retained.is_activatable && incoming.can_retain_previous(&retained) {
                                            executable = prior.executable;
                                            chosen = retained;
                                        }
                                    }
                                }
                            }
                        }
                        let permitted = self.http_generations[&executable].demand(Some(&plan.signature.policy));
                        selection.insert(
                            key.clone(),
                            Selection {
                                raw_key: raw_key.clone(),
                                latest,
                                executable,
                                permitted,
                                activatable: chosen.is_activatable,
                            },
                        );
                        scope.http.insert(key.clone(), chosen);
                    }
                    for (key, (_, stages)) in &plan.signature.stream {
                        if let Some(generation) = self.stream_generations.get_mut(key) {
                            let same = old_signature.is_some_and(|old| {
                                old.available
                                    && old.policy == plan.signature.policy
                                    && old.stream.get(key) == plan.signature.stream.get(key)
                            });
                            let old = old_scope.and_then(|scope| scope.stream.get(key));
                            let view = match old.filter(|_| same) {
                                Some(view) => Arc::clone(view),
                                None => generation.compile(&plan.signature.policy, *stages),
                            };
                            scope.stream.insert(key.clone(), view);
                        }
                    }
                    for route in &plan.routes {
                        scope.rules.insert(
                            Arc::as_ptr(route) as usize,
                            rule_admission(route, &scope, &self.http_sources),
                        );
                    }
                    scope.has_access_log_extern = scope.http.values().any(|view| {
                        view.spec
                            .spec
                            .access_log_extern
                            .as_ref()
                            .is_some_and(|fields| !fields.is_empty())
                    });
                }
                Arc::new(scope)
            };
            for choice in selection.values() {
                http_demand
                    .entry(choice.executable)
                    .or_default()
                    .extend(choice.permitted.iter().copied());
            }
            if scope.available {
                for (key, (_, stages)) in &plan.signature.stream {
                    if let Some(generation) = self.stream_generations.get(key) {
                        stream_demand
                            .entry(key.clone())
                            .or_default()
                            .extend(&generation.demand(&plan.signature.policy, *stages));
                    }
                }
            }
            next.scopes.insert(class.clone(), scope);
            selected.insert(class.clone(), selection);
            signatures.insert(class, plan.signature);
        }
        self.selected = selected;
        self.signatures = signatures;
        let current: HashSet<u64> = self.http_latest.values().copied().collect();
        self.http_generations.retain(|id, generation| {
            generation.release_unneeded_entries(http_demand.get(id).unwrap_or(&EntryDemand::new()));
            current.contains(id) || http_demand.contains_key(id)
        });
        for (key, generation) in &mut self.stream_generations {
            generation.release_unneeded_entries(stream_demand.get(key).unwrap_or(&StreamEntryDemand::default()));
        }
        self.compose_listeners(previous, &mut next);
        next.stats = PublicationStats {
            scopes: next.scopes.len(),
            http_generations: self.http_generations.len(),
            http_cached_entries: self
                .http_generations
                .values()
                .map(PluginGeneration::cached_entry_count)
                .sum(),
            stream_cached_entries: self
                .stream_generations
                .values()
                .map(StreamPluginGeneration::cached_entry_count)
                .sum(),
            ai_route_tokens: next.scopes.values().map(|scope| scope.rules.len()).sum(),
            listeners: next.listeners.len(),
        };
        next.inventory.scopes.extend(next.scopes.keys().cloned());
        for (id, generation) in &self.http_generations {
            let source = generation.source();
            let key = format!(
                "{}/{}@{id}",
                source.metadata.namespace.as_deref().unwrap_or("default"),
                source.metadata.name.as_deref().unwrap_or("")
            );
            next.inventory.http_generations.push(key.clone());
            next.inventory.http_entries.extend(
                generation
                    .cached_entry_keys()
                    .map(|slot| format!("{key}/{:?}/{}", slot.stage, slot.index)),
            );
        }
        for (key, generation) in &self.stream_generations {
            let id = self.stream_ids[key];
            next.inventory.stream_entries.extend(
                generation
                    .cached_entry_keys()
                    .into_iter()
                    .map(|(tls, index)| format!("{key}@{id}/{}/{index}", if tls { "TlsRoute" } else { "Connection" })),
            );
        }
        for (class, scope) in &next.scopes {
            for token in scope.rules.values() {
                if let Some(route) = token.source.upgrade() {
                    next.inventory.ai_routes.push(format!(
                        "{class}/{}/{}",
                        route.metadata.namespace.as_deref().unwrap_or("default"),
                        route.metadata.name.as_deref().unwrap_or("")
                    ));
                }
            }
        }
        next.inventory.scopes.sort();
        next.inventory.http_generations.sort();
        next.inventory.http_entries.sort();
        next.inventory.stream_entries.sort();
        next.inventory.ai_routes.sort();
        next
    }

    fn compose_listeners(&self, previous: &PluginPublication, next: &mut PluginPublication) {
        let mut entries: HashMap<ListenerKey, Vec<(String, ListenerEntry)>> = HashMap::new();
        for (key, gateway) in &self.gateways {
            let Some(scope) = next.scopes.get(&gateway.class_name) else {
                continue;
            };
            let gateway_scope = match previous
                .gateways
                .get(key)
                .filter(|old| Arc::ptr_eq(&old.scope, scope) && old.http_keys == gateway.http_keys)
            {
                Some(old) => Arc::clone(old),
                None => Arc::new(GatewayScope {
                    gateway_key: key.clone(),
                    scope: Arc::clone(scope),
                    http_keys: gateway.http_keys.clone(),
                }),
            };
            next.gateways.insert(key.clone(), gateway_scope);
            let (namespace, name) = key.split_once('/').unwrap_or(("default", key.as_str()));
            for listener in &gateway.listeners {
                next.listeners
                    .entry(listener.key.clone())
                    .or_default()
                    .push(key.clone());
                next.index_signatures
                    .entry(listener.key.clone())
                    .or_default()
                    .push((key.clone(), self.gateway_versions.get(key).copied().unwrap_or_default()));
                // This index resolves ownership only. The existing route context
                // continues to own Gateway metrics, SNI policy and DNS metadata.
                let info = Arc::new(GatewayInfo::new(
                    Some(namespace.to_owned()),
                    name.to_owned(),
                    Some(listener.name.clone()),
                    None,
                    None,
                    None,
                    gateway.http_keys.clone(),
                    None,
                ));
                entries.entry(listener.key.clone()).or_default().push((
                    key.clone(),
                    ListenerEntry {
                        port: listener.port,
                        info,
                        gateway_proof: None,
                        hostname: listener.hostname.clone(),
                        allowed_routes: listener.allowed_routes.clone(),
                    },
                ));
            }
        }
        for keys in next.listeners.values_mut() {
            keys.sort();
            keys.dedup();
        }
        for (key, signature) in &mut next.index_signatures {
            signature.sort();
            let old = previous
                .indices
                .get(key)
                .filter(|_| previous.index_signatures.get(key) == Some(signature));
            let index = match old {
                Some(old) => Arc::clone(old),
                None => Arc::new(GatewayInfoIndex::from_entries(entries.remove(key).unwrap_or_default())),
            };
            next.indices.insert(key.clone(), index);
        }
        for (listener, keys) in &next.listeners {
            next.connections
                .insert(listener.clone(), self.connection_view(keys, next));
        }
    }

    fn connection_view(&self, keys: &[String], publication: &PluginPublication) -> ConnectionView {
        if keys.iter().all(|key| self.gateways[key].connection_key.is_none()) {
            return ConnectionView::Empty;
        }
        let mut common = None;
        let mut signature = None;
        for key in keys {
            let gateway = &self.gateways[key];
            let scope = &publication.gateways[key].scope;
            if !scope.available {
                return ConnectionView::Unavailable;
            }
            let mut current = Vec::new();
            if let Some(resource_key) = &gateway.connection_key {
                if resource_key.is_empty() {
                    return ConnectionView::Unavailable;
                }
                let Some(source) = self.stream_sources.get(resource_key) else {
                    return ConnectionView::Unavailable;
                };
                let Some(view) = scope.stream.get(resource_key) else {
                    return ConnectionView::Unavailable;
                };
                let policy = &self.signatures[&gateway.class_name].policy;
                for (index, entry) in source
                    .spec
                    .plugins
                    .iter()
                    .flatten()
                    .enumerate()
                    .filter(|(_, entry)| entry.is_enabled())
                {
                    let action = policy.action_for(PluginFamily::Stream, entry.type_name());
                    if action != Some(PluginPolicyAction::Bypass) {
                        current.push((resource_key.as_str(), self.stream_ids[resource_key], index, action));
                    }
                }
                if !current.is_empty() {
                    common = Some(Arc::clone(view));
                }
            }
            match &signature {
                Some(previous) if previous != &current => return ConnectionView::Unavailable,
                None => signature = Some(current),
                _ => {}
            }
        }
        common.map_or(ConnectionView::Empty, ConnectionView::Ready)
    }
}

fn rule_admission(
    route: &Arc<HTTPRoute>,
    scope: &PluginScope,
    sources: &HashMap<String, Arc<EdgionPlugins>>,
) -> RuleAdmission {
    let namespace = route.metadata.namespace.as_deref().unwrap_or("default");
    let count = route.effective_rules().map_or(0, |rules| rules.len());
    let mut allowed = vec![false; count];
    let mut graphql_allowed = vec![false; count];
    let Ok(references) = HttpRouteExtensionRefs::from_route(route) else {
        return RuleAdmission {
            source: Arc::downgrade(route),
            allowed,
            graphql_allowed,
        };
    };
    for (index, rule) in effective_http_rules(route) {
        let mut graphql_plugins = Vec::new();
        let mut graphql_sources = Vec::new();
        let mut active_count = 0;
        let mut configs = Vec::new();
        let mut missing = None;
        for key in rule
            .filters
            .iter()
            .flatten()
            .enumerate()
            .filter_map(|(filter, _)| references.effective_at(index, None, filter))
            .filter_map(extension_ref_key)
        {
            if let Ok(PluginTarget::Unique { source, .. }) = classify(&key, |raw| sources.get(raw).cloned()) {
                graphql_sources.push(source);
            }
            match scope.http.get(&key) {
                Some(view) => {
                    graphql_plugins.push(view.spec.as_ref());
                    for entry in view
                        .spec
                        .spec
                        .request_plugins
                        .iter()
                        .flatten()
                        .filter(|entry| entry.is_enabled())
                    {
                        if let EdgionPlugin::AiProxy(config) = &entry.plugin {
                            active_count += 1;
                            configs.push(config);
                        }
                    }
                }
                None => missing = Some(key),
            }
        }
        graphql_allowed[index] = edgion_resources::plugin_configs::graphql_router_rule_binding_error(
            rule,
            namespace,
            &graphql_sources.iter().map(|source| source.as_ref()).collect::<Vec<_>>(),
            route.spec.resolved_rules.is_some(),
        )
        .is_none()
            && edgion_resources::plugin_configs::graphql_router_rule_error(
                rule,
                namespace,
                &graphql_plugins,
                route.spec.resolved_rules.is_some(),
            )
            .is_none();
        allowed[index] = ai_rule_invalid_reason(
            rule,
            namespace,
            rule.resolved_ai_admission.as_ref(),
            AiProxyAdmission {
                active_count,
                configs: &configs,
                missing_reference: missing.as_deref(),
            },
        )
        .is_none();
    }
    RuleAdmission {
        source: Arc::downgrade(route),
        allowed,
        graphql_allowed,
    }
}
