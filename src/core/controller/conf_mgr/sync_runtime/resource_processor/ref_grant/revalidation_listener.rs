//! Revalidation Listener Implementation
//!
//! Listens for ReferenceGrant changes and triggers revalidation of affected resources.

use super::events::{ReferenceGrantChangedEvent, RevalidationListener};
use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::core::controller::conf_mgr::ControllerRuntime;
use std::sync::Arc;

/// Trigger revalidation for all resources with cross-namespace references
///
/// This should be called after all processors are ready during Init phase.
/// It ensures that any Route resources that were processed before their
/// corresponding ReferenceGrant resources are revalidated.
pub async fn trigger_full_cross_ns_revalidation(runtime: Arc<ControllerRuntime>) {
    let manager = &runtime.cross_ns_ref_mgr;
    let stats = manager.stats();

    if stats.value_count == 0 {
        tracing::debug!(
            component = "cross_ns_revalidation",
            "No resources with cross-namespace references, skipping full revalidation"
        );
        return;
    }

    tracing::info!(
        component = "cross_ns_revalidation",
        resource_count = stats.value_count,
        target_namespace_count = stats.source_count,
        total_references = stats.total_references,
        "Triggering full cross-namespace revalidation after init"
    );

    let mut requeued = std::collections::HashSet::new();
    let mut requeued_count = 0;

    for ns in manager.all_source_keys() {
        for resource_ref in manager.get_refs(&ns) {
            let ref_key = resource_ref.key();
            if !requeued.contains(&ref_key) {
                runtime
                    .processor_registry
                    .requeue(resource_ref.kind_str(), resource_ref.resource_key())
                    .await;
                requeued.insert(ref_key);
                requeued_count += 1;
            }
        }
    }

    if requeued_count > 0 {
        tracing::info!(
            component = "cross_ns_revalidation",
            requeued_count = requeued_count,
            "Completed full cross-namespace revalidation"
        );
    }
}

/// Requeue all routes registered in the gateway_route_index after init.
///
/// During init, routes may register in gateway_route_index via on_change, but
/// if a route's on_change runs before its parent Gateway's on_change, the
/// Gateway's hostname/port change detection fires when no routes are registered
/// yet. This one-time requeue ensures all such routes get reprocessed with
/// the Gateway already in cache, so lookup_gateway() in parse() succeeds.
pub async fn trigger_gateway_route_revalidation(runtime: Arc<ControllerRuntime>) {
    let routes = runtime.gateway_route_index.all_routes();
    if routes.is_empty() {
        return;
    }

    tracing::info!(
        component = "gateway_route_revalidation",
        route_count = routes.len(),
        "Requeuing all gateway-referencing routes for post-init revalidation"
    );

    for (kind, key) in routes {
        runtime.processor_registry.requeue(kind.as_str(), key).await;
    }
}

/// Requeue all Gateways after init to resolve TLS certificate references.
///
/// During init, Gateways may be processed before Secrets are loaded into the
/// SecretStore (each resource type initializes independently). This function
/// requeues all Gateways so they re-evaluate ResolvedRefs with fully populated stores.
pub async fn trigger_gateway_secret_revalidation(runtime: Arc<ControllerRuntime>) {
    let Some(gateway_proc) = runtime.processor_registry.get("Gateway") else {
        return;
    };

    let keys = gateway_proc.list_keys();
    if keys.is_empty() {
        return;
    }

    tracing::info!(
        component = "gateway_secret_revalidation",
        gateway_count = keys.len(),
        "Requeuing all Gateways for post-init TLS secret revalidation"
    );

    for key in keys {
        runtime.processor_registry.requeue("Gateway", key).await;
    }
}

/// Run the standard post-cache-ready revalidation sequence.
///
/// Called once all processors are ready (or after timeout) to ensure
/// cross-namespace refs, TLS secrets, and parentRef routes are consistent.
/// Order matters: cross-ns grants first, then secrets, then routes.
pub async fn run_post_cache_ready_revalidations(runtime: Arc<ControllerRuntime>) {
    trigger_full_cross_ns_revalidation(runtime.clone()).await;
    trigger_gateway_secret_revalidation(runtime.clone()).await;
    trigger_gateway_route_revalidation(runtime.clone()).await;
    trigger_gatewayclass_params_revalidation(runtime).await;
}

/// Requeue all GatewayClasses after caches are ready.
///
/// A GatewayClass's ResolvedRefs/Accepted conditions depend on whether its referenced
/// EdgionGatewayConfig exists. During init the GatewayClass may be processed before its EGC
/// (the init path does not fire `on_change`), so this one-shot revalidation re-resolves
/// parametersRef once everything is loaded. Runs in both Kubernetes and file/etcd modes.
pub async fn trigger_gatewayclass_params_revalidation(runtime: Arc<ControllerRuntime>) {
    let Some(gc_processor) = runtime.processor_registry.get("GatewayClass") else {
        return;
    };

    let keys = gc_processor.list_keys();
    if keys.is_empty() {
        return;
    }

    tracing::info!(
        component = "gatewayclass_params_revalidation",
        gateway_class_count = keys.len(),
        "Requeuing all GatewayClasses for post-init parametersRef revalidation"
    );

    for key in keys {
        runtime.processor_registry.requeue("GatewayClass", key).await;
    }
}

/// Cross-namespace revalidation listener
///
/// When ReferenceGrant changes, finds all resources that have cross-namespace
/// references to the affected namespaces and requeues them for revalidation.
pub struct CrossNsRevalidationListener {
    runtime: Arc<ControllerRuntime>,
}

impl CrossNsRevalidationListener {
    pub fn new(runtime: Arc<ControllerRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl RevalidationListener for CrossNsRevalidationListener {
    async fn on_reference_grant_changed(&self, event: &ReferenceGrantChangedEvent) {
        let manager = &self.runtime.cross_ns_ref_mgr;

        let namespaces_to_check: Vec<String> = if event.affected_namespaces.is_empty() {
            tracing::info!(
                component = "cross_ns_revalidation",
                "ReferenceGrant changed with no specific affected namespaces, triggering full revalidation"
            );
            manager.all_source_keys()
        } else {
            event.affected_namespaces.iter().cloned().collect()
        };

        if namespaces_to_check.is_empty() {
            return;
        }

        // Build a chain origin so downstream cascades inherit TriggerChain
        // cycle detection. Without this, resources requeued here would start
        // with an empty chain and any further cascading requeues would bypass
        // the cycle limit entirely.
        //
        // The key is a sorted, comma-joined list of affected namespaces so
        // the same logical event produces the same chain ID across runs,
        // making log correlation reliable. HashSet iteration order is
        // non-deterministic and would otherwise yield a different chain ID
        // every time.
        let origin_key = if event.affected_namespaces.is_empty() {
            "*".to_string()
        } else {
            let mut keys: Vec<&str> = event.affected_namespaces.iter().map(String::as_str).collect();
            keys.sort_unstable();
            keys.join(",")
        };
        let origin_chain = TriggerChain::new().extend("ReferenceGrant", &origin_key);

        let mut requeued = std::collections::HashSet::new();
        let mut requeued_count = 0;

        for ns in &namespaces_to_check {
            let refs = manager.get_refs(ns);

            for resource_ref in refs {
                let ref_key = resource_ref.key();
                if requeued.contains(&ref_key) {
                    continue;
                }

                let kind = resource_ref.kind_str();
                let key = resource_ref.resource_key();

                tracing::debug!(
                    component = "cross_ns_revalidation",
                    affected_namespace = %ns,
                    kind = %kind,
                    key = %key,
                    "Requeuing resource due to ReferenceGrant change"
                );

                self.runtime
                    .processor_registry
                    .requeue_with_chain(kind, key, origin_chain.clone())
                    .await;
                requeued.insert(ref_key);
                requeued_count += 1;
            }
        }

        if requeued_count > 0 {
            tracing::info!(
                component = "cross_ns_revalidation",
                affected_ns_count = namespaces_to_check.len(),
                requeued_count = requeued_count,
                "Requeued resources due to ReferenceGrant change"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ref_manager::ResourceRef;
    use edgion_resources::ResourceKind;
    use std::collections::HashSet;

    #[test]
    fn test_listener_finds_affected_resources() {
        let runtime = Arc::new(ControllerRuntime::new());
        let manager = &runtime.cross_ns_ref_mgr;

        manager.clear();

        let route = ResourceRef::new(ResourceKind::HTTPRoute, Some("app".to_string()), "my-route".to_string());
        manager.add_ref("backend".to_string(), route);

        let _event = ReferenceGrantChangedEvent {
            affected_namespaces: {
                let mut set = HashSet::new();
                set.insert("backend".to_string());
                set
            },
        };

        let refs = manager.get_refs("backend");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].resource_key(), "app/my-route");
        assert_eq!(refs[0].kind_str(), "HTTPRoute");

        manager.clear();
    }
}
