//! ControllerRuntime — aggregates all shared mutable state for the Conf Center.
//!
//! Owned by KubernetesCenter / FileSystemCenter and threaded down through
//! SpawnContext → ResourceProcessor → HandlerContext to every handler call.
//! `reset()` provides a centralized clear point used on leader-change or
//! controller restart. All state is held as struct fields (compiler-tracked).

use std::collections::HashMap;

use super::processor_registry::ProcessorRegistry;
use super::sync_runtime::resource_processor::{
    attached_route_tracker::AttachedRouteTracker,
    backend_tls_policy_ref::BackendTLSPolicyState,
    configmap_utils::ConfigMapStore,
    gateway_route_index::GatewayRouteIndex,
    listener_port_manager::ListenerPortManager,
    namespace_store::NamespaceStore,
    ref_grant::{
        cross_ns_ref_manager::{new_cross_ns_ref_manager, CrossNamespaceRefManager},
        events::ReferenceGrantEventDispatcher,
        ReferenceGrantStore,
    },
    secret_utils::SecretStore,
    service_ref::{new_service_ref_manager, ServiceRefManager},
};

/// Aggregated runtime state for the Controller.
///
/// All fields are plain (non-Arc) values; the struct itself is shared as
/// `Arc<ControllerRuntime>` so cloning is cheap and all owners see the same data.
pub struct ControllerRuntime {
    pub(crate) processor_registry: ProcessorRegistry,
    pub(crate) namespace_store: NamespaceStore,
    pub(crate) ref_grant_dispatcher: ReferenceGrantEventDispatcher,
    pub(crate) ref_grant_store: ReferenceGrantStore,
    pub(crate) cross_ns_ref_mgr: CrossNamespaceRefManager,
    pub(crate) service_ref_mgr: ServiceRefManager,
    pub(crate) secret_store: SecretStore,
    pub(crate) gateway_route_index: GatewayRouteIndex,
    pub(crate) attached_route_tracker: AttachedRouteTracker,
    pub(crate) listener_port_manager: ListenerPortManager,
    pub(crate) configmap_store: ConfigMapStore,
    pub(crate) backend_tls_policy: BackendTLSPolicyState,
}

impl ControllerRuntime {
    pub fn new() -> Self {
        Self {
            processor_registry: ProcessorRegistry::new(),
            namespace_store: NamespaceStore::new(),
            ref_grant_dispatcher: ReferenceGrantEventDispatcher::new(),
            ref_grant_store: ReferenceGrantStore::new(),
            cross_ns_ref_mgr: new_cross_ns_ref_manager(),
            service_ref_mgr: new_service_ref_manager(),
            secret_store: SecretStore::new(),
            gateway_route_index: GatewayRouteIndex::new(),
            attached_route_tracker: AttachedRouteTracker::new(),
            listener_port_manager: ListenerPortManager::new(),
            configmap_store: ConfigMapStore::new(),
            backend_tls_policy: BackendTLSPolicyState::new(),
        }
    }

    /// Clear all mutable state for leader-change or controller restart.
    ///
    /// **Not cleared** (intentional):
    /// - `ref_grant_dispatcher` listeners: registered once at process startup in
    ///   `ConfMgr::create()`; listeners are stateless and hold `Arc<ControllerRuntime>`,
    ///   so they remain valid across resets. Clearing them would permanently break
    ///   ReferenceGrant revalidation because nothing re-registers them.
    /// - `NamespaceStore`: self-heals via `Event::InitDone` on reconnect.
    /// - `ListenerPortManager`: re-populated when Gateway reconcile runs after restart.
    ///
    /// **Arc cycles**: `processor_registry` holds `Arc<ResourceProcessor>` which holds
    /// `Arc<ControllerRuntime>`, and `ref_grant_dispatcher` holds `Arc<CrossNsRevalidationListener>`
    /// which holds `Arc<ControllerRuntime>`. Both cycles are intentional: `ControllerRuntime`
    /// is process-scoped and never dropped. `clear_processors()` breaks the first cycle
    /// on each restart; dispatcher listeners are left alive by design (see above).
    pub fn reset(&self) {
        self.processor_registry.clear_processors();
        tracing::info!(component = "controller_runtime", "Cleared ProcessorRegistry");

        self.service_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared ServiceRefManager");

        self.ref_grant_store.replace_all(HashMap::new());
        tracing::info!(component = "controller_runtime", "Cleared ReferenceGrantStore");

        self.cross_ns_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared CrossNamespaceRefManager");

        self.gateway_route_index.clear();
        tracing::info!(component = "controller_runtime", "Cleared GatewayRouteIndex");

        self.attached_route_tracker.clear();
        tracing::info!(component = "controller_runtime", "Cleared AttachedRouteTracker");

        self.secret_store.replace_all(HashMap::new());
        tracing::info!(component = "controller_runtime", "Cleared SecretStore");

        self.configmap_store.clear();
        tracing::info!(component = "controller_runtime", "Cleared ConfigMapStore");

        self.backend_tls_policy.reset();
    }
}

impl Default for ControllerRuntime {
    fn default() -> Self {
        Self::new()
    }
}
