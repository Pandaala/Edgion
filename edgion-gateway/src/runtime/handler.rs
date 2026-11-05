use crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store;
use crate::conf_sync::conf_client::scoped_handler::SharedScopeRegistry;
use crate::routes::grpc::get_global_grpc_route_managers;
use crate::routes::http::get_global_http_route_managers;
use crate::routes::tcp::get_global_tcp_route_managers;
use crate::routes::tls::get_global_tls_route_managers;
use crate::routes::udp::get_global_udp_route_managers;
use crate::runtime::matching::rebuild_gateway_tls_matcher;
use crate::runtime::server::proxy_protocol_policy::{
    get_global_proxy_protocol_policy_runtime, ProxyProtocolPolicyRuntime, SnapshotDisposition,
};
use crate::runtime::store::{
    get_global_gateway_store, get_listener_gateway_info_store, rebuild_listener_gateway_infos, GatewayStore,
};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::ListenerKey;
use edgion_resources::ResourceMeta;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// ConfHandler implementation for Gateway.
///
/// Handles both initial full-set sync and incremental updates (add/update/remove).
/// This handler manages two stores:
/// 1. GatewayStore: Raw Gateway resources
/// 2. GatewayTlsMatcher: Port-based TLS certificate matching
///
/// Listener-level routing state is rebuilt into `ListenerGatewayInfoStore`
/// (see `rebuild_listener_gateway_infos`); per-listener route managers are
/// rebuilt when physical keys or effective serving listener membership change.
pub struct GatewayHandler {
    policy_runtime: Arc<ProxyProtocolPolicyRuntime>,
    gateway_scope: Option<String>,
    gateway_uid: Option<String>,
    shared: Arc<SharedScopeRegistry>,
}

impl GatewayHandler {
    pub fn new() -> Self {
        Self::new_scoped(None)
    }

    pub fn new_scoped(gateway_scope: Option<String>) -> Self {
        Self::new_with_identity(gateway_scope, None, Arc::default())
    }

    pub(crate) fn new_with_identity(
        gateway_scope: Option<String>,
        gateway_uid: Option<String>,
        shared: Arc<SharedScopeRegistry>,
    ) -> Self {
        Self {
            policy_runtime: get_global_proxy_protocol_policy_runtime(),
            gateway_scope,
            gateway_uid,
            shared,
        }
    }
}

fn gateway_in_scope(scope: Option<&str>, uid: Option<&str>, key: &str, gateway: &Gateway) -> bool {
    match scope {
        Some(scope) => scope == key && uid.is_none_or(|uid| gateway.metadata.uid.as_deref() == Some(uid)),
        None => !is_dedicated_gateway(gateway),
    }
}

fn is_dedicated_gateway(gateway: &Gateway) -> bool {
    gateway
        .metadata
        .annotations
        .as_ref()
        .and_then(|value| value.get(edgion_resources::constants::annotations::edgion::DEDICATED_GATEWAY_INSTANCE))
        .is_some_and(|value| value == "true")
}

impl ConfHandler<Gateway> for GatewayHandler {
    fn full_set(&self, data: &HashMap<String, Arc<Gateway>>) {
        let dedicated: HashSet<String> = data
            .iter()
            .filter(|(_, gateway)| is_dedicated_gateway(gateway))
            .map(|(key, _)| key.clone())
            .collect();
        let scoped_data: HashMap<String, Arc<Gateway>> = data
            .iter()
            .filter(|(key, gateway)| {
                gateway_in_scope(self.gateway_scope.as_deref(), self.gateway_uid.as_deref(), key, gateway)
            })
            .map(|(key, gateway)| (key.clone(), Arc::clone(gateway)))
            .collect();
        let data = &scoped_data;
        if matches!(
            self.policy_runtime.observe_snapshot(data.values().map(Arc::as_ref)),
            SnapshotDisposition::RejectFatal
        ) {
            return;
        }

        tracing::info!(
            count = data.len(),
            "Gateway full_set: received {} Gateway resources",
            data.len()
        );

        let eligibility_before = serving_listener_inventory(&get_global_gateway_store().read().list_gateways());
        let declared_before = get_global_gateway_store().read().declared_http_listener_keys();
        let gateways = {
            let global_store = get_global_gateway_store();
            let mut store = global_store.write();
            for old in store.list_gateways() {
                get_global_gateway_backend_tls_store().remove(&old.key_name());
            }
            store.clear();

            for (key, gateway) in data {
                let listener_count = gateway.spec.listeners.as_ref().map(|l| l.len()).unwrap_or(0);
                tracing::info!(
                    key = %key,
                    namespace = ?gateway.namespace(),
                    name = %gateway.name_any(),
                    gateway_class = %gateway.spec.gateway_class_name,
                    listeners = listener_count,
                    "Gateway stored"
                );

                update_gateway_backend_tls_store(key, gateway);

                // store was just cleared and `data` keys are unique by HashMap construction,
                // so a duplicate-key error from add_gateway is unreachable here. Use the
                // idempotent update_gateway to avoid carrying a dead warn branch.
                store.update_gateway((**gateway).clone());
            }

            store.list_gateways()
        }; // write lock on gateway store released here

        // Rebuild Gateway TLS matcher (port-based certificate lookup)
        rebuild_gateway_tls_matcher(&gateways);

        // Rebuild listener → GatewayInfo mapping (for dynamic route matching)
        let keys_before = get_listener_gateway_info_store().all_listener_keys();
        rebuild_listener_gateway_infos(&gateways);
        let keys_after = get_listener_gateway_info_store().all_listener_keys();

        // Rebuild per-listener route managers (HTTP/gRPC/TCP/TLS/UDP) only when
        // the listener set changed (listeners added or removed). Without rebuilding
        // TCP/TLS/UDP here, their per-listener entries leak when a Gateway is
        // removed: route delete only checks active_listeners (ListenerGatewayInfoStore)
        // while Gateway still exists, so empty buckets are not removed; once Gateway
        // disappears there is no other trigger to clean up the now-orphan listener entries.
        let declared_after = get_global_gateway_store().read().declared_http_listener_keys();
        if eligibility_before != serving_listener_inventory(&gateways)
            || declared_before != declared_after
            || listeners_changed(&keys_before, &keys_after)
        {
            get_global_http_route_managers().rebuild_all_listener_managers();
            get_global_grpc_route_managers().rebuild_all_listener_managers();
        }
        if listeners_changed(&keys_before, &keys_after) {
            get_global_tcp_route_managers().rebuild_all_listener_managers();
            get_global_tls_route_managers().rebuild_all_listener_managers();
            get_global_udp_route_managers().rebuild_all_listener_managers();
        }
        // Cached delegation children prebuild leaf metric handles for every
        // Gateway in the process; recompile them when that set changed.
        get_global_http_route_managers().refresh_delegation_gateways();
        crate::routes::grpc::get_global_grpc_route_managers().refresh_delegation_gateways();
        // Full listener entries and scoped policy publish together, including
        // same-key SNI/DNS/proof changes that do not rebuild route tables.
        crate::plugins::publication::global_publisher().replace_gateways(
            data.iter()
                .map(|(key, gateway)| {
                    (
                        key.clone(),
                        crate::plugins::publication::inputs::GatewayInput::from_gateway(gateway),
                    )
                })
                .collect(),
        );
        crate::ai::reconcile_all_adaptive_plugin_dependents();
        if self.gateway_scope.is_none() {
            self.shared.replace(dedicated);
        } else {
            self.shared.set_active(!data.is_empty());
        }
    }

    fn partial_update(
        &self,
        mut add: HashMap<String, Arc<Gateway>>,
        mut update: HashMap<String, Arc<Gateway>>,
        mut remove: HashSet<String>,
    ) {
        let mut dedicated = self.shared.snapshot();
        if self.gateway_scope.is_none() {
            for (key, gateway) in add.iter().chain(update.iter()) {
                if is_dedicated_gateway(gateway) {
                    dedicated.insert(key.clone());
                } else {
                    dedicated.remove(key);
                }
            }
            for key in &remove {
                dedicated.remove(key);
            }
        }
        if let Some(scope) = self.gateway_scope.as_deref() {
            if add.iter().chain(update.iter()).any(|(key, gateway)| {
                key == scope && !gateway_in_scope(Some(scope), self.gateway_uid.as_deref(), key, gateway)
            }) {
                remove.insert(scope.to_string());
            }
            add.retain(|key, gateway| gateway_in_scope(Some(scope), self.gateway_uid.as_deref(), key, gateway));
            update.retain(|key, gateway| gateway_in_scope(Some(scope), self.gateway_uid.as_deref(), key, gateway));
            remove.retain(|key| key == scope);
        } else {
            for (key, gateway) in add.iter().chain(update.iter()) {
                if is_dedicated_gateway(gateway) {
                    remove.insert(key.clone());
                }
            }
            add.retain(|_, gateway| !is_dedicated_gateway(gateway));
            update.retain(|_, gateway| !is_dedicated_gateway(gateway));
        }
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, gateway)| {
                (
                    key.clone(),
                    crate::plugins::publication::inputs::GatewayInput::from_gateway(gateway),
                )
            })
            .collect();
        let policy_removed = remove.clone();
        if !add.is_empty() || !update.is_empty() || !remove.is_empty() {
            let prospective = prospective_gateway_snapshot(&add, &update, &remove);
            if matches!(
                self.policy_runtime.observe_snapshot(prospective.values()),
                SnapshotDisposition::RejectFatal
            ) {
                return;
            }
        }

        let mut needs_rebuild = false;

        // Mirror `full_set`: keep all GatewayStore mutations inside a scoped block
        // and snapshot `list_gateways()` before dropping the write guard. The
        // rebuild functions below do not read GatewayStore, so holding the lock
        // across them would only stall unrelated GatewayStore consumers.
        let eligibility_before = serving_listener_inventory(&get_global_gateway_store().read().list_gateways());
        let declared_before = get_global_gateway_store().read().declared_http_listener_keys();
        let gateways = {
            let global_store = get_global_gateway_store();
            let mut store = global_store.write();

            if !add.is_empty() {
                tracing::info!(
                    count = add.len(),
                    "Gateway partial_update: added {} Gateway resources",
                    add.len()
                );
                for (key, gateway) in add {
                    let listener_count = gateway.spec.listeners.as_ref().map(|l| l.len()).unwrap_or(0);
                    tracing::info!(
                        key = %key,
                        namespace = ?gateway.namespace(),
                        name = %gateway.name_any(),
                        gateway_class = %gateway.spec.gateway_class_name,
                        listeners = listener_count,
                        "Gateway added"
                    );

                    update_gateway_backend_tls_store(&key, &gateway);

                    store_added_gateway(&mut store, (*gateway).clone());
                }
                needs_rebuild = true;
            }

            if !update.is_empty() {
                tracing::info!(
                    count = update.len(),
                    "Gateway partial_update: updated {} Gateway resources",
                    update.len()
                );
                for (key, gateway) in update {
                    let listener_count = gateway.spec.listeners.as_ref().map(|l| l.len()).unwrap_or(0);
                    tracing::info!(
                        key = %key,
                        namespace = ?gateway.namespace(),
                        name = %gateway.name_any(),
                        gateway_class = %gateway.spec.gateway_class_name,
                        listeners = listener_count,
                        // Routes and TLS certificates hot-reload here; listener port
                        // add/remove does NOT (sockets are bound once at startup, see
                        // the rebuild_listener_gateway_infos comment below).
                        "Gateway updated (routes/TLS hot-reloaded; listener port \
                         add/remove requires a process restart)"
                    );

                    update_gateway_backend_tls_store(&key, &gateway);

                    store.update_gateway((*gateway).clone());
                }
                needs_rebuild = true;
            }

            if !remove.is_empty() {
                tracing::info!(
                    count = remove.len(),
                    "Gateway partial_update: removed {} Gateway resources",
                    remove.len()
                );
                for key in &remove {
                    tracing::info!(
                        key = %key,
                        "Gateway removed"
                    );

                    get_global_gateway_backend_tls_store().remove(key);

                    if let Err(e) = store.remove_gateway(key) {
                        tracing::warn!(key = %key, error = %e, "Failed to remove gateway");
                    }
                }
                needs_rebuild = true;
            }

            if needs_rebuild {
                Some(store.list_gateways())
            } else {
                None
            }
        }; // write lock on gateway store released here

        if let Some(gateways) = gateways {
            // GatewayTlsMatcher and ListenerGatewayInfoStore are two independent
            // ArcSwap-backed globals; we rebuild them sequentially rather than
            // committing them as a single atomic snapshot. This leaves a brief
            // window (between the two stores below) in which a concurrent TLS
            // handshake can match against the new SNI table while subsequent
            // listener routing still reads the previous ListenerGatewayInfoStore
            // epoch — and vice versa. Accepted as a design tradeoff: the window
            // is microseconds long on a cold path (controller→gateway sync),
            // self-heals on the next request, and merging both into one
            // ArcSwap<Snapshot> would require pulling unrelated state behind a
            // single global lifetime with significant ripple through the route
            // managers. See `skills/04-review/concurrency/runtime-F005-*` for
            // the rationale and the criteria that would justify revisiting this.
            rebuild_gateway_tls_matcher(&gateways);

            // KNOWN LIMITATION: this rebuilds only the routing data structures
            // (ListenerGatewayInfoStore + per-listener route managers). The OS-level listener sockets
            // are bound once during startup (create_and_configure_server ->
            // configure_listeners) and the Server is then consumed by
            // Server::run(RunArgs) (cli/pingora.rs), so we hold no handle to
            // bind/drain sockets here. Adding a new listener port to a live Gateway updates
            // routing but leaves the new port unbound (traffic silently dropped);
            // removing a port leaves its socket bound with no route. A port-set
            // change therefore requires a gateway process restart. TLS certificate
            // updates are exempt — they hot-reload via ArcSwap on existing sockets.
            // See skills/01-architecture/02-gateway/01-startup-shutdown.md.
            let keys_before = get_listener_gateway_info_store().all_listener_keys();
            rebuild_listener_gateway_infos(&gateways);
            let keys_after = get_listener_gateway_info_store().all_listener_keys();

            let declared_after = get_global_gateway_store().read().declared_http_listener_keys();
            if eligibility_before != serving_listener_inventory(&gateways)
                || declared_before != declared_after
                || listeners_changed(&keys_before, &keys_after)
            {
                get_global_http_route_managers().rebuild_all_listener_managers();
                get_global_grpc_route_managers().rebuild_all_listener_managers();
            }
            if listeners_changed(&keys_before, &keys_after) {
                get_global_tcp_route_managers().rebuild_all_listener_managers();
                get_global_tls_route_managers().rebuild_all_listener_managers();
                get_global_udp_route_managers().rebuild_all_listener_managers();
            }
            // Cached delegation children prebuild leaf metric handles for
            // every Gateway in the process; recompile them when that set changed.
            get_global_http_route_managers().refresh_delegation_gateways();
            crate::routes::grpc::get_global_grpc_route_managers().refresh_delegation_gateways();
        }
        // GatewayInput carries the canonical full listener entries; index and
        // scope change in one publication even when physical keys are unchanged.
        crate::plugins::publication::global_publisher().update_gateways(policy_inputs, &policy_removed);
        crate::ai::reconcile_all_adaptive_plugin_dependents();
        if self.gateway_scope.is_none() {
            self.shared.replace(dedicated);
        } else if let Some(scope) = self.gateway_scope.as_deref() {
            let active = get_global_gateway_store()
                .read()
                .list_gateways()
                .iter()
                .any(|gateway| gateway.key_name() == scope);
            self.shared.set_active(active);
        }
    }
}

/// Compressed Add carries the final value, even when the runtime key exists.
/// Match the prospective snapshot and the backend TLS/plugin publication inputs.
fn store_added_gateway(store: &mut GatewayStore, gateway: Gateway) {
    store.update_gateway(gateway);
}

fn prospective_gateway_snapshot(
    add: &HashMap<String, Arc<Gateway>>,
    update: &HashMap<String, Arc<Gateway>>,
    remove: &HashSet<String>,
) -> HashMap<String, Gateway> {
    let mut prospective: HashMap<String, Gateway> = get_global_gateway_store()
        .read()
        .list_gateways()
        .into_iter()
        .map(|gateway| (gateway.key_name(), gateway))
        .collect();
    for key in remove {
        prospective.remove(key);
    }
    for (key, gateway) in add.iter().chain(update) {
        prospective.insert(key.clone(), (**gateway).clone());
    }
    prospective
}

/// Update GatewayBackendTlsStore from a Gateway's resolved client certificate.
fn update_gateway_backend_tls_store(gateway_key: &str, gateway: &Gateway) {
    get_global_gateway_backend_tls_store().update_from_gateway(gateway_key, gateway);
}

/// Check whether the set of listener keys changed between two snapshots.
/// Order-independent comparison using sorted vecs.
fn serving_listener_inventory(gateways: &[Gateway]) -> std::collections::BTreeSet<(String, String, ListenerKey)> {
    gateways
        .iter()
        .flat_map(|gateway| {
            crate::runtime::store::listener_gateway_info::gateway_listener_entries(gateway)
                .into_iter()
                .map(|(key, entry)| {
                    (
                        gateway.key_name(),
                        entry.info.listener_name.clone().unwrap_or_default(),
                        key,
                    )
                })
        })
        .collect()
}

fn listeners_changed(before: &[ListenerKey], after: &[ListenerKey]) -> bool {
    if before.len() != after.len() {
        return true;
    }
    let mut a = before.to_vec();
    let mut b = after.to_vec();
    a.sort();
    b.sort();
    a != b
}

pub fn create_gateway_handler() -> Box<dyn ConfHandler<Gateway> + Send + Sync> {
    Box::new(GatewayHandler::new())
}

pub(crate) fn create_scoped_gateway_handler(
    scope: Option<String>,
    uid: Option<String>,
    shared: Arc<SharedScopeRegistry>,
) -> Box<dyn ConfHandler<Gateway> + Send + Sync> {
    Box::new(GatewayHandler::new_with_identity(scope, uid, shared))
}

#[cfg(test)]
#[path = "../../tests/unit/runtime/handler_test.rs"]
mod tests;
