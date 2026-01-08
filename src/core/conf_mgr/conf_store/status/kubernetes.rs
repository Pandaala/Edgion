//! Kubernetes StatusStore implementation
//!
//! Updates resource status via Kubernetes API using Server-Side Apply.

use async_trait::async_trait;
use kube::api::{Patch, PatchParams};
use kube::{Api, Client};
use serde_json::json;

use super::{StatusStore, StatusStoreError};
use crate::types::resources::gateway::{Gateway, GatewayStatus};
use crate::types::resources::http_route::{HTTPRoute, HTTPRouteStatus};

/// Kubernetes-based status store
///
/// Uses Server-Side Apply to update status subresources via the K8s API.
pub struct KubernetesStatusStore {
    client: Client,
    field_manager: String,
}

impl KubernetesStatusStore {
    /// Create a new Kubernetes status store
    ///
    /// # Arguments
    /// * `client` - Kubernetes client
    /// * `field_manager` - Field manager name for Server-Side Apply
    pub fn new(client: Client, field_manager: String) -> Self {
        Self { client, field_manager }
    }
}

#[async_trait]
impl StatusStore for KubernetesStatusStore {
    async fn update_gateway_status(
        &self,
        namespace: &str,
        name: &str,
        status: GatewayStatus,
    ) -> Result<(), StatusStoreError> {
        let api: Api<Gateway> = Api::namespaced(self.client.clone(), namespace);

        let patch = Patch::Apply(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "Gateway",
            "metadata": {
                "name": name,
                "namespace": namespace,
            },
            "status": status
        }));

        let params = PatchParams::apply(&self.field_manager).force();
        api.patch_status(name, &params, &patch)
            .await
            .map_err(|e| StatusStoreError::KubeError(e.to_string()))?;

        tracing::debug!(
            component = "k8s_status_store",
            kind = "Gateway",
            namespace = namespace,
            name = name,
            "Gateway status updated"
        );

        Ok(())
    }

    async fn update_http_route_status(
        &self,
        namespace: &str,
        name: &str,
        status: HTTPRouteStatus,
    ) -> Result<(), StatusStoreError> {
        let api: Api<HTTPRoute> = Api::namespaced(self.client.clone(), namespace);

        let patch = Patch::Apply(json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "name": name,
                "namespace": namespace,
            },
            "status": status
        }));

        let params = PatchParams::apply(&self.field_manager).force();
        api.patch_status(name, &params, &patch)
            .await
            .map_err(|e| StatusStoreError::KubeError(e.to_string()))?;

        tracing::debug!(
            component = "k8s_status_store",
            kind = "HTTPRoute",
            namespace = namespace,
            name = name,
            "HTTPRoute status updated"
        );

        Ok(())
    }

    async fn get_gateway_status(&self, namespace: &str, name: &str) -> Result<Option<GatewayStatus>, StatusStoreError> {
        let api: Api<Gateway> = Api::namespaced(self.client.clone(), namespace);

        match api.get(name).await {
            Ok(gateway) => Ok(gateway.status),
            Err(kube::Error::Api(err)) if err.code == 404 => Ok(None),
            Err(e) => Err(StatusStoreError::KubeError(e.to_string())),
        }
    }

    async fn get_http_route_status(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<HTTPRouteStatus>, StatusStoreError> {
        let api: Api<HTTPRoute> = Api::namespaced(self.client.clone(), namespace);

        match api.get(name).await {
            Ok(route) => Ok(route.status),
            Err(kube::Error::Api(err)) if err.code == 404 => Ok(None),
            Err(e) => Err(StatusStoreError::KubeError(e.to_string())),
        }
    }
}
