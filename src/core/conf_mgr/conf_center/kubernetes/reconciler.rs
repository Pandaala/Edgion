//! Status Reconciler
//!
//! Provides utility methods for Status generation and update.
//! In event-driven mode, these methods are called directly by the controller
//! when resources change, rather than via a polling loop.
//!
//! Note: The polling loop has been removed. Status updates are now event-driven
//! and handled in controller.rs via handle_gateway_event and handle_http_route_event.

use crate::core::conf_mgr::conf_center::StatusStore;
use crate::core::conf_mgr::resource_check;
use crate::core::conf_sync::ConfigServer;
use crate::types::resources::gateway::Gateway;
use crate::types::resources::http_route::HTTPRoute;
use kube::ResourceExt;
use std::sync::Arc;

/// StatusReconciler provides utility methods for status management.
///
/// In the current event-driven architecture, status updates are triggered
/// immediately when resources change in the controller. This struct is kept
/// for potential future use (e.g., manual reconciliation, startup sync).
#[allow(dead_code)]
pub struct StatusReconciler {
    config_server: Arc<ConfigServer>,
    status_store: Arc<dyn StatusStore>,
    gateway_class_name: String,
}

impl StatusReconciler {
    /// Create a new StatusReconciler
    pub fn new(
        config_server: Arc<ConfigServer>,
        status_store: Arc<dyn StatusStore>,
        gateway_class_name: String,
    ) -> Self {
        Self {
            config_server,
            status_store,
            gateway_class_name,
        }
    }

    /// Update status for a single Gateway (called by controller on resource change)
    #[allow(dead_code)]
    pub async fn update_gateway_status(&self, gateway: &Gateway) {
        if let Some(status) = resource_check::generate_gateway_status(gateway, &self.gateway_class_name) {
            let ns = gateway.namespace().unwrap_or_else(|| "default".to_string());
            let name = gateway.name_any();

            if let Err(e) = self.status_store.update_gateway_status(&ns, &name, status).await {
                tracing::error!(
                    component = "status_reconciler",
                    kind = "Gateway",
                    name = %name,
                    namespace = %ns,
                    error = %e,
                    "Failed to update Gateway status"
                );
            } else {
                tracing::debug!(
                    component = "status_reconciler",
                    kind = "Gateway",
                    name = %name,
                    namespace = %ns,
                    "Gateway status updated"
                );
            }
        }
    }

    /// Update status for a single HTTPRoute (called by controller on resource change)
    #[allow(dead_code)]
    pub async fn update_http_route_status(&self, route: &HTTPRoute) {
        if let Some(status) = resource_check::generate_http_route_status(route) {
            let ns = route.namespace().unwrap_or_else(|| "default".to_string());
            let name = route.name_any();

            if let Err(e) = self.status_store.update_http_route_status(&ns, &name, status).await {
                tracing::error!(
                    component = "status_reconciler",
                    kind = "HTTPRoute",
                    name = %name,
                    namespace = %ns,
                    error = %e,
                    "Failed to update HTTPRoute status"
                );
            } else {
                tracing::debug!(
                    component = "status_reconciler",
                    kind = "HTTPRoute",
                    name = %name,
                    namespace = %ns,
                    "HTTPRoute status updated"
                );
            }
        }
    }

    /// Sync all Gateway statuses (for startup or manual reconciliation)
    #[allow(dead_code)]
    pub async fn sync_all_gateway_statuses(&self) {
        let gateways = self.config_server.gateways.list_owned();

        for gateway in gateways.data {
            self.update_gateway_status(&gateway).await;
        }

        tracing::info!(component = "status_reconciler", "All Gateway statuses synced");
    }

    /// Sync all HTTPRoute statuses (for startup or manual reconciliation)
    #[allow(dead_code)]
    pub async fn sync_all_http_route_statuses(&self) {
        let routes = self.config_server.routes.list_owned();

        for route in routes.data {
            self.update_http_route_status(&route).await;
        }

        tracing::info!(component = "status_reconciler", "All HTTPRoute statuses synced");
    }
}
