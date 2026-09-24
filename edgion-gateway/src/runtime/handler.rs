use crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store;
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
/// only rebuilt when the listener key set actually changes.
pub struct GatewayHandler {
    policy_runtime: Arc<ProxyProtocolPolicyRuntime>,
}

impl GatewayHandler {
    pub fn new() -> Self {
        Self {
            policy_runtime: get_global_proxy_protocol_policy_runtime(),
        }
    }
}

impl ConfHandler<Gateway> for GatewayHandler {
    fn full_set(&self, data: &HashMap<String, Arc<Gateway>>) {
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

        let gateways = {
            let global_store = get_global_gateway_store();
            let mut store = global_store.write();
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
        if listeners_changed(&keys_before, &keys_after) {
            get_global_http_route_managers().rebuild_all_listener_managers();
            get_global_grpc_route_managers().rebuild_all_listener_managers();
            get_global_tcp_route_managers().rebuild_all_listener_managers();
            get_global_tls_route_managers().rebuild_all_listener_managers();
            get_global_udp_route_managers().rebuild_all_listener_managers();
        }
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
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<Gateway>>,
        update: HashMap<String, Arc<Gateway>>,
        remove: HashSet<String>,
    ) {
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

            if listeners_changed(&keys_before, &keys_after) {
                get_global_http_route_managers().rebuild_all_listener_managers();
                get_global_grpc_route_managers().rebuild_all_listener_managers();
                get_global_tcp_route_managers().rebuild_all_listener_managers();
                get_global_tls_route_managers().rebuild_all_listener_managers();
                get_global_udp_route_managers().rebuild_all_listener_managers();
            }
        }
        crate::plugins::publication::global_publisher().update_gateways(policy_inputs, &policy_removed);
        crate::ai::reconcile_all_adaptive_plugin_dependents();
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
    let store = get_global_gateway_backend_tls_store();
    let has_cert = gateway
        .spec
        .tls
        .as_ref()
        .and_then(|t| t.backend.as_ref())
        .and_then(|b| b.resolved_client_certificate.as_ref());

    if let Some(secret) = has_cert {
        store.set(gateway_key.to_string(), Arc::new(secret.clone()));
    } else {
        store.remove(gateway_key);
    }
}

/// Check whether the set of listener keys changed between two snapshots.
/// Order-independent comparison using sorted vecs.
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

#[cfg(test)]
#[path = "../../tests/unit/runtime/handler_test.rs"]
mod tests;
