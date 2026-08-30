//! ControllerRuntime — aggregates all shared mutable state for the Conf Center.
//!
//! Owned by KubernetesCenter / FileSystemCenter and threaded down through
//! SpawnContext → ResourceProcessor → HandlerContext to every handler call.
//! `reset()` provides a centralized clear point used on leader-change or
//! controller restart. All state is held as struct fields (compiler-tracked).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use super::conf_center::CenterApi;
use super::processor_registry::ProcessorRegistry;
use super::sync_runtime::resource_processor::{
    attached_route_tracker::AttachedRouteTracker,
    backend_tls_policy_ref::BackendTLSPolicyState,
    configmap_utils::ConfigMapStore,
    credential_source_state::CredentialSourceRegistry,
    edgion_config_data_utils::{EdgionConfigDataRefManager, EdgionConfigDataStore},
    gateway_route_index::GatewayRouteIndex,
    listener_port_manager::ListenerPortManager,
    namespace_store::NamespaceStore,
    ref_grant::{
        cross_ns_ref_manager::{new_cross_ns_ref_manager, CrossNamespaceRefManager},
        events::ReferenceGrantEventDispatcher,
        ReferenceGrantStore,
    },
    route_delegation_index::RouteDelegationIndex,
    secret_utils::{new_secret_ref_manager, SecretRefManager, SecretStore},
    service_ref::{new_service_ref_manager, ServiceRefManager},
    BidirectionalRefManager, ResourceRef,
};
use crate::services::credential_manager::memory::CredentialMemoryStore;

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
    /// Tracks HTTPRoutes and EdgionBackendTrafficPolicies that reference each
    /// EdgionBackend. Source keys use `PolicyTargetKey::to_string()` so backend
    /// targets cannot collide with same-named core Services.
    pub(crate) backend_ref_mgr: BidirectionalRefManager<ResourceRef>,
    pub(crate) secret_ref_manager: SecretRefManager,
    pub(crate) secret_store: SecretStore,
    pub(crate) gateway_route_index: GatewayRouteIndex,
    /// child HTTPRoute → delegating parent HTTPRoutes reverse index; a child
    /// change requeues its parents for delegation re-flattening.
    pub(crate) route_delegation_index: RouteDelegationIndex,
    /// child GRPCRoute → delegating parent GRPCRoutes reverse index. Kept as
    /// a separate `RouteDelegationIndex` instance (same type, reused
    /// unchanged) so HTTPRoute and GRPCRoute `"ns/name"` keys never collide
    /// and a GRPCRoute change only requeues gRPC parents.
    pub(crate) grpc_route_delegation_index: RouteDelegationIndex,
    pub(crate) attached_route_tracker: AttachedRouteTracker,
    pub(crate) listener_port_manager: ListenerPortManager,
    pub(crate) configmap_store: ConfigMapStore,
    pub(crate) backend_tls_policy: BackendTLSPolicyState,
    /// Target-conflict + creation-timestamp index for `EdgionBackendTrafficPolicy`.
    /// Reuses `BackendTLSPolicyState` (a generic target-conflict/precedence store)
    /// to implement the same oldest-wins conflict detection and sibling
    /// re-evaluation the `BackendTLSPolicy` handler uses. Kept separate from
    /// `backend_tls_policy` so the two resource kinds never collide on a shared
    /// Service key.
    pub(crate) edgion_backend_traffic_policy: BackendTLSPolicyState,
    pub(crate) edgion_config_data_store: EdgionConfigDataStore,
    /// Tracks which EdgionPlugins/EdgionStreamPlugins reference each EdgionConfigData.
    /// Used by the EdgionConfigDataHandler to requeue referencing resources on change.
    pub(crate) edgion_config_data_refs: EdgionConfigDataRefManager,
    /// Tracks which Gateways reference each EdgionStreamPlugins via
    /// `edgion.io/edgion-stream-plugins` annotation.
    /// Forward: `"esp_ns/esp_name" → Set<Gateway ResourceRef>`
    /// Reverse: `Gateway ResourceRef → Set<"esp_ns/esp_name">`
    pub(crate) stream_plugin_ref_mgr: BidirectionalRefManager<ResourceRef>,
    /// Tracks which Gateways, HTTPRoutes, and GRPCRoutes reference each EdgionPlugins.
    ///
    /// Gateways use the `edgion.io/edgion-plugins` annotation; Routes use
    /// rule- and backend-level EdgionPlugins ExtensionRefs. Forward:
    /// `"ep_ns/ep_name" → Set<ResourceRef>`; reverse: owner → Set<key>.
    pub(crate) plugin_ref_mgr: BidirectionalRefManager<ResourceRef>,
    /// Tracks EdgionPlugins consumers of each LinkSys credential source.
    pub(crate) link_sys_ref_mgr: BidirectionalRefManager<ResourceRef>,
    /// Non-secret desired source and publication bindings rebuilt by handlers.
    pub(crate) credential_source_registry: CredentialSourceRegistry,
    /// Per-LinkSys in-memory generated credentials. Never attached to LinkSys.
    pub(crate) credential_memory: CredentialMemoryStore,
    managed_credentials_supported: AtomicBool,
    /// True when this replica is the sole credential publisher (leader-only or
    /// a non-Kubernetes single-process center). False under Kubernetes all-serve.
    single_publisher: AtomicBool,
    /// Optional conf_writer injected by the ConfCenter implementation.
    /// Used by handlers to auto-create resources (e.g., EdgionConfigData for RegionRoute).
    /// None until the ConfCenter calls `set_conf_writer()`.
    pub(crate) conf_writer: RwLock<Option<Arc<dyn CenterApi + Send + Sync>>>,
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
            backend_ref_mgr: BidirectionalRefManager::with_component("backend_ref_mgr"),
            secret_ref_manager: new_secret_ref_manager(),
            secret_store: SecretStore::new(),
            gateway_route_index: GatewayRouteIndex::new(),
            route_delegation_index: RouteDelegationIndex::new(),
            grpc_route_delegation_index: RouteDelegationIndex::new(),
            attached_route_tracker: AttachedRouteTracker::new(),
            listener_port_manager: ListenerPortManager::new(),
            configmap_store: ConfigMapStore::new(),
            backend_tls_policy: BackendTLSPolicyState::new(),
            edgion_backend_traffic_policy: BackendTLSPolicyState::new(),
            edgion_config_data_store: EdgionConfigDataStore::new(),
            edgion_config_data_refs: EdgionConfigDataRefManager::new(),
            stream_plugin_ref_mgr: BidirectionalRefManager::with_component("stream_plugin_ref_mgr"),
            plugin_ref_mgr: BidirectionalRefManager::with_component("plugin_ref_mgr"),
            link_sys_ref_mgr: BidirectionalRefManager::with_component("link_sys_ref_mgr"),
            credential_source_registry: CredentialSourceRegistry::new(),
            credential_memory: CredentialMemoryStore::new(),
            managed_credentials_supported: AtomicBool::new(true),
            single_publisher: AtomicBool::new(true),
            conf_writer: RwLock::new(None),
        }
    }

    pub fn set_managed_credentials_supported(&self, supported: bool) {
        self.managed_credentials_supported.store(supported, Ordering::Release);
    }

    pub fn managed_credentials_supported(&self) -> bool {
        self.managed_credentials_supported.load(Ordering::Acquire)
    }

    pub fn set_single_publisher(&self, single_publisher: bool) {
        self.single_publisher.store(single_publisher, Ordering::Release);
    }

    pub fn single_publisher(&self) -> bool {
        self.single_publisher.load(Ordering::Acquire)
    }

    /// Inject a conf_writer so handlers can auto-create resources.
    ///
    /// Called once by the ConfCenter after construction.
    pub fn set_conf_writer(&self, writer: Arc<dyn CenterApi + Send + Sync>) {
        *self.conf_writer.write() = Some(writer);
    }

    /// Get the conf_writer (if available).
    pub fn conf_writer(&self) -> Option<Arc<dyn CenterApi + Send + Sync>> {
        self.conf_writer.read().clone()
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

        self.backend_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared BackendRefManager");

        self.secret_ref_manager.clear();
        tracing::info!(component = "controller_runtime", "Cleared SecretRefManager");

        self.ref_grant_store.replace_all(HashMap::new());
        tracing::info!(component = "controller_runtime", "Cleared ReferenceGrantStore");

        self.cross_ns_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared CrossNamespaceRefManager");

        self.gateway_route_index.clear();
        tracing::info!(component = "controller_runtime", "Cleared GatewayRouteIndex");

        self.route_delegation_index.clear();
        tracing::info!(component = "controller_runtime", "Cleared RouteDelegationIndex");

        self.grpc_route_delegation_index.clear();
        tracing::info!(
            component = "controller_runtime",
            "Cleared GRPCRoute RouteDelegationIndex"
        );

        self.attached_route_tracker.clear();
        tracing::info!(component = "controller_runtime", "Cleared AttachedRouteTracker");

        self.secret_store.replace_all(HashMap::new());
        tracing::info!(component = "controller_runtime", "Cleared SecretStore");

        self.configmap_store.clear();
        tracing::info!(component = "controller_runtime", "Cleared ConfigMapStore");

        self.backend_tls_policy.reset();
        self.edgion_backend_traffic_policy.reset();

        self.edgion_config_data_store.replace_all(HashMap::new());
        tracing::info!(component = "controller_runtime", "Cleared EdgionConfigData store");

        self.edgion_config_data_refs.clear_all();
        tracing::info!(component = "controller_runtime", "Cleared EdgionConfigData ref manager");

        self.stream_plugin_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared StreamPluginRefManager");

        self.plugin_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared PluginRefManager");

        self.link_sys_ref_mgr.clear();
        tracing::info!(component = "controller_runtime", "Cleared LinkSysRefManager");

        self.credential_source_registry.clear();
        tracing::info!(component = "controller_runtime", "Cleared CredentialSourceRegistry");

        self.credential_memory.clear();
        tracing::info!(component = "controller_runtime", "Cleared CredentialMemoryStore");
    }
}

impl Default for ControllerRuntime {
    fn default() -> Self {
        Self::new()
    }
}
