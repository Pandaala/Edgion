//! KubernetesWriter implementation
//!
//! Implements the ConfWriter trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use crate::core::conf_mgr::conf_center::{ConfEntry, ConfWriter, ConfWriterError, ListOptions, ListResult};
use anyhow::Result;
use async_trait::async_trait;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams};
use kube::core::{DynamicObject, GroupVersionKind};
use kube::{Api, Client, Discovery};

/// Kubernetes API based configuration writer
///
/// Implements ConfWriter by calling K8s API server.
/// Uses dynamic client to support any resource type.
pub struct KubernetesWriter {
    client: Client,
}

impl KubernetesWriter {
    /// Create a new KubernetesWriter with default client
    pub async fn new() -> Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self { client })
    }

    /// Create a new KubernetesWriter with existing client
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// Get the Kubernetes client
    pub fn client(&self) -> &Client {
        &self.client
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
            "EdgionPlugins" => GroupVersionKind::gvk("edgion.io", "v1", "EdgionPlugins"),
            "EdgionStreamPlugins" => GroupVersionKind::gvk("edgion.io", "v1", "EdgionStreamPlugins"),
            "BackendTLSPolicy" => GroupVersionKind::gvk("gateway.networking.k8s.io", "v1alpha3", "BackendTLSPolicy"),
            "PluginMetaData" => GroupVersionKind::gvk("edgion.io", "v1", "PluginMetaData"),
            "LinkSys" => GroupVersionKind::gvk("edgion.io", "v1", "LinkSys"),
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
    ///
    /// # Arguments
    /// * `kind` - Resource kind
    /// * `namespace` - For namespaced resources: Some(ns) = specific namespace, None = default namespace
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

    /// Create a dynamic API that lists resources across ALL namespaces
    ///
    /// For cluster-scoped resources, behaves the same as `dynamic_api`.
    /// For namespaced resources, uses `Api::all_with()` to list across all namespaces.
    async fn dynamic_api_all_namespaces(
        &self,
        kind: &str,
    ) -> Result<(Api<DynamicObject>, GroupVersionKind), ConfWriterError> {
        let gvk = self.resolve_gvk(kind).await?;

        // Discover API resource
        let discovery = Discovery::new(self.client.clone())
            .run()
            .await
            .map_err(|e| ConfWriterError::KubeError(format!("Discovery failed: {}", e)))?;

        let (ar, _caps) = discovery
            .resolve_gvk(&gvk)
            .ok_or_else(|| ConfWriterError::KubeError(format!("GVK not found: {:?}", gvk)))?;

        // Always use Api::all_with to list across all namespaces
        let api: Api<DynamicObject> = Api::all_with(self.client.clone(), &ar);

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

        let obj = api.get(name).await.map_err(|e| match e {
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
        api.delete(name, &params).await.map_err(|e| match e {
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

    async fn list_all(&self, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        // list_all requires iterating all known resource kinds
        // This is expensive, but we support it for compatibility
        //
        // Note: continue_token is NOT supported for list_all across multiple kinds.
        // Only limit is supported (truncates final result).
        if let Some(ref options) = opts {
            if options.continue_token.is_some() {
                tracing::warn!(
                    component = "kubernetes_writer",
                    "list_all does not support continue_token, it will be ignored"
                );
            }
        }

        let all_kinds = [
            "Gateway",
            "GatewayClass",
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "UDPRoute",
            "TLSRoute",
            "ReferenceGrant",
            "Secret",
            "Service",
            "Endpoints",
            "EndpointSlice",
            "EdgionTls",
            "EdgionGatewayConfig",
            "EdgionPlugins",
            "EdgionStreamPlugins",
            "BackendTLSPolicy",
            "PluginMetaData",
            "LinkSys",
        ];

        let mut all_items = Vec::new();

        for kind in &all_kinds {
            // Use None for opts to get all items from each kind
            match self.get_list_by_kind(kind, None).await {
                Ok(result) => {
                    all_items.extend(result.items);
                }
                Err(e) => {
                    // Log but continue - some resources may not exist in cluster
                    tracing::debug!(
                        component = "kubernetes_writer",
                        kind = kind,
                        error = %e,
                        "Failed to list resources of kind, skipping"
                    );
                }
            }
        }

        // Apply pagination locally if opts is provided
        let (items, continue_token) = if let Some(ref options) = opts {
            if options.limit > 0 {
                let limit = options.limit as usize;
                if all_items.len() > limit {
                    let items = all_items.into_iter().take(limit).collect();
                    // Note: For list_all, we don't support true pagination across kinds
                    // Just truncate the result
                    (items, None)
                } else {
                    (all_items, None)
                }
            } else {
                (all_items, None)
            }
        } else {
            (all_items, None)
        };

        Ok(ListResult {
            items,
            continue_token,
        })
    }

    async fn get_list_by_kind(&self, kind: &str, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        // Use dynamic_api_all_namespaces to list across ALL namespaces
        let (api, _gvk) = self.dynamic_api_all_namespaces(kind).await?;

        let mut lp = ListParams::default();

        // Apply pagination options if provided
        if let Some(ref options) = opts {
            if options.limit > 0 {
                lp = lp.limit(options.limit);
            }
            if let Some(ref token) = options.continue_token {
                lp = lp.continue_token(token);
            }
        }

        let list = api
            .list(&lp)
            .await
            .map_err(|e| ConfWriterError::KubeError(format!("Failed to list {}: {}", kind, e)))?;

        let items = list
            .items
            .into_iter()
            .map(|obj| ConfEntry {
                kind: kind.to_string(),
                namespace: obj.metadata.namespace.clone(),
                name: obj.metadata.name.clone().unwrap_or_default(),
                content: serde_yaml::to_string(&obj).unwrap_or_default(),
            })
            .collect();

        tracing::debug!(
            component = "kubernetes_writer",
            kind = kind,
            count = list.metadata.remaining_item_count,
            "Listed resources by kind (all namespaces)"
        );

        Ok(ListResult {
            items,
            continue_token: list.metadata.continue_,
        })
    }

    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult, ConfWriterError> {
        let (api, _gvk) = self.dynamic_api(kind, Some(namespace)).await?;

        let mut lp = ListParams::default();

        // Apply pagination options if provided
        if let Some(ref options) = opts {
            if options.limit > 0 {
                lp = lp.limit(options.limit);
            }
            if let Some(ref token) = options.continue_token {
                lp = lp.continue_token(token);
            }
        }

        let list = api
            .list(&lp)
            .await
            .map_err(|e| ConfWriterError::KubeError(format!("Failed to list {}/{}: {}", kind, namespace, e)))?;

        let items = list
            .items
            .into_iter()
            .map(|obj| ConfEntry {
                kind: kind.to_string(),
                namespace: Some(namespace.to_string()),
                name: obj.metadata.name.clone().unwrap_or_default(),
                content: serde_yaml::to_string(&obj).unwrap_or_default(),
            })
            .collect();

        tracing::debug!(
            component = "kubernetes_writer",
            kind = kind,
            namespace = namespace,
            count = list.metadata.remaining_item_count,
            "Listed resources by kind and namespace"
        );

        Ok(ListResult {
            items,
            continue_token: list.metadata.continue_,
        })
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfWriterError> {
        // Note: K8s API doesn't have a count-only endpoint, so we fetch all items.
        // This may be slow for large resource sets.
        // Consider using Controller's Store for faster access if available.
        let result = self.get_list_by_kind(kind, None).await?;
        Ok(result.items.len())
    }

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfWriterError> {
        // Note: K8s API doesn't have a count-only endpoint, so we fetch all items.
        let result = self.get_list_by_kind_ns(kind, namespace, None).await?;
        Ok(result.items.len())
    }
}
