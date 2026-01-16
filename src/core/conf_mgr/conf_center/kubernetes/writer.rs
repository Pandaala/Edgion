//! KubernetesWriter implementation
//!
//! Implements the ConfWriter trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use crate::core::conf_mgr::conf_center::{ConfEntry, ConfWriter, ConfWriterError};
use anyhow::Result;
use async_trait::async_trait;
use kube::api::{DeleteParams, Patch, PatchParams};
use kube::core::{DynamicObject, GroupVersionKind};
use kube::{Api, Client, Discovery};
use std::sync::Arc;

use super::KubernetesStore;

/// Kubernetes API based configuration writer
///
/// Implements ConfWriter by calling K8s API server.
/// Uses dynamic client to support any resource type.
pub struct KubernetesWriter {
    client: Client,
    /// Reference to the store for cache operations
    store: Arc<KubernetesStore>,
}

impl KubernetesWriter {
    /// Create a new KubernetesWriter with default client
    ///
    /// Returns both the writer and the store reference (for Controller use)
    pub async fn new() -> Result<(Self, Arc<KubernetesStore>)> {
        let client = Client::try_default().await?;
        let store = KubernetesStore::with_client(client.clone());
        Ok((Self { client, store: store.clone() }, store))
    }

    /// Create a new KubernetesWriter with existing client and store
    pub fn with_client_and_store(client: Client, store: Arc<KubernetesStore>) -> Self {
        Self { client, store }
    }

    /// Get the Kubernetes client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get the store reference
    pub fn store(&self) -> &Arc<KubernetesStore> {
        &self.store
    }

    /// Resolve GVK for a resource kind
    async fn resolve_gvk(&self, kind: &str) -> Result<GroupVersionKind, ConfWriterError> {
        // Map common kinds to their GVK
        let gvk = match kind {
            "Gateway" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "Gateway"),
            "GatewayClass" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "GatewayClass"),
            "HTTPRoute" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "HTTPRoute"),
            "GRPCRoute" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "GRPCRoute"),
            "TCPRoute" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1alpha2", "TCPRoute"),
            "UDPRoute" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1alpha2", "UDPRoute"),
            "TLSRoute" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1alpha2", "TLSRoute"),
            "ReferenceGrant" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1beta1", "ReferenceGrant"),
            "Secret" => GroupVersionKind::gvk("", "v1", "Secret"),
            "Service" => GroupVersionKind::gvk("", "v1", "Service"),
            "Endpoints" => GroupVersionKind::gvk("", "v1", "Endpoints"),
            "EndpointSlice" => GroupVersionKind::gvk("discovery.k8s.io", "v1", "EndpointSlice"),
            "EdgionTls" => GroupVersionKind::gvk("edgion.io", "v1", "EdgionTls"),
            "EdgionGatewayConfig" => GroupVersionKind::gvk("edgion.io", "v1", "EdgionGatewayConfig"),
            _ => {
                return Err(ConfWriterError::InternalError(format!(
                    "Unknown resource kind: {}",
                    kind
                )))
            }
        };
        Ok(gvk)
    }

    /// Create a dynamic API for the given kind and namespace
    async fn dynamic_api(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Result<(Api<DynamicObject>, GroupVersionKind), ConfWriterError> {
        let gvk = self.resolve_gvk(kind).await?;

        // Discover API resource
        let discovery = Discovery::new(self.client.clone())
            .run()
            .await
            .map_err(|e| ConfWriterError::KubeError(format!("Discovery failed: {}", e)))?;

        let (ar, caps) = discovery
            .resolve_gvk(&gvk)
            .ok_or_else(|| ConfWriterError::KubeError(format!("GVK not found: {:?}", gvk)))?;

        let api: Api<DynamicObject> = if caps.scope == kube::discovery::Scope::Namespaced {
            let ns = namespace.unwrap_or("default");
            Api::namespaced_with(self.client.clone(), ns, &ar)
        } else {
            Api::all_with(self.client.clone(), &ar)
        };

        Ok((api, gvk))
    }
}

#[async_trait]
impl ConfWriter for KubernetesWriter {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let (api, _gvk) = self.dynamic_api(kind, namespace).await?;

        // Parse content as DynamicObject
        let obj: DynamicObject = serde_yaml::from_str(&content)
            .map_err(|e| ConfWriterError::ParseError(format!("Failed to parse YAML: {}", e)))?;

        // Use server-side apply
        let params = PatchParams::apply("edgion-controller").force();
        api.patch(name, &params, &Patch::Apply(&obj))
            .await
            .map_err(|e| ConfWriterError::KubeError(format!("Failed to apply resource: {}", e)))?;

        tracing::info!(
            component = "kubernetes_writer",
            event = "resource_applied",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource applied via K8s API"
        );

        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        let (api, _gvk) = self.dynamic_api(kind, namespace).await?;

        let obj = api
            .get(name)
            .await
            .map_err(|e| match e {
                kube::Error::Api(ae) if ae.code == 404 => {
                    ConfWriterError::NotFound(format!("{}/{}/{}", kind, namespace.unwrap_or("_"), name))
                }
                _ => ConfWriterError::KubeError(format!("Failed to get resource: {}", e)),
            })?;

        let content = serde_yaml::to_string(&obj)
            .map_err(|e| ConfWriterError::ParseError(format!("Failed to serialize: {}", e)))?;

        Ok(content)
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        let (api, _gvk) = self.dynamic_api(kind, namespace).await?;

        let params = DeleteParams::default();
        api.delete(name, &params)
            .await
            .map_err(|e| match e {
                kube::Error::Api(ae) if ae.code == 404 => {
                    ConfWriterError::NotFound(format!("{}/{}/{}", kind, namespace.unwrap_or("_"), name))
                }
                _ => ConfWriterError::KubeError(format!("Failed to delete resource: {}", e)),
            })?;

        tracing::info!(
            component = "kubernetes_writer",
            event = "resource_deleted",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource deleted via K8s API"
        );

        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<ConfEntry>, ConfWriterError> {
        // For K8s mode, list_all returns from the in-memory cache
        // The cache is populated by the Controller's watch
        Ok(self.store.list_all().await)
    }

    async fn get_list_by_kind(&self, kind: &str) -> Result<Vec<ConfEntry>, ConfWriterError> {
        // For K8s mode, use the in-memory cache
        Ok(self.store.list_by_kind(kind).await)
    }

    async fn get_list_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<Vec<ConfEntry>, ConfWriterError> {
        // For K8s mode, filter from cache
        let all = self.store.list_by_kind(kind).await;
        Ok(all
            .into_iter()
            .filter(|e| e.namespace.as_deref() == Some(namespace))
            .collect())
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError> {
        Ok(self.store.list_by_kind(kind).await.len())
    }

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError> {
        let all = self.store.list_by_kind(kind).await;
        Ok(all
            .iter()
            .filter(|e| e.namespace.as_deref() == Some(namespace))
            .count())
    }
}
