//! KubernetesWriter implementation
//!
//! Implements the ConfWriter trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use crate::core::conf_mgr::conf_center::{ConfEntry, ConfWriter, ConfWriterError, ListOptions, ListResult};
use anyhow::Result;
use async_trait::async_trait;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::discovery::Scope;
use kube::{Api, Client};

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

    /// Resolve ApiResource and Scope for a resource kind (static mapping, no network call)
    fn resolve_api_resource(&self, kind: &str) -> Result<(ApiResource, Scope), ConfWriterError> {
        // Static mapping of resource kinds to their ApiResource and Scope
        // This avoids expensive Discovery calls on every API operation
        let (ar, scope) = match kind {
            // Gateway API resources (namespaced)
            "Gateway" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "Gateway")),
                Scope::Namespaced,
            ),
            "GatewayClass" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1",
                    "GatewayClass",
                )),
                Scope::Cluster,
            ),
            "HTTPRoute" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "HTTPRoute")),
                Scope::Namespaced,
            ),
            "GRPCRoute" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "GRPCRoute")),
                Scope::Namespaced,
            ),
            "TCPRoute" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1alpha2",
                    "TCPRoute",
                )),
                Scope::Namespaced,
            ),
            "UDPRoute" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1alpha2",
                    "UDPRoute",
                )),
                Scope::Namespaced,
            ),
            "TLSRoute" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1alpha2",
                    "TLSRoute",
                )),
                Scope::Namespaced,
            ),
            "ReferenceGrant" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1beta1",
                    "ReferenceGrant",
                )),
                Scope::Namespaced,
            ),
            "BackendTLSPolicy" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1alpha3",
                    "BackendTLSPolicy",
                )),
                Scope::Namespaced,
            ),
            // Core resources
            "Secret" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Secret")),
                Scope::Namespaced,
            ),
            "Service" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Service")),
                Scope::Namespaced,
            ),
            "Endpoints" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Endpoints")),
                Scope::Namespaced,
            ),
            "EndpointSlice" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("discovery.k8s.io", "v1", "EndpointSlice")),
                Scope::Namespaced,
            ),
            // Edgion CRDs
            "EdgionTls" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "EdgionTls")),
                Scope::Namespaced,
            ),
            "EdgionGatewayConfig" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "EdgionGatewayConfig")),
                Scope::Cluster,
            ),
            "EdgionPlugins" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "EdgionPlugins")),
                Scope::Namespaced,
            ),
            "EdgionStreamPlugins" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "EdgionStreamPlugins")),
                Scope::Namespaced,
            ),
            "PluginMetaData" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "PluginMetaData")),
                Scope::Namespaced,
            ),
            "LinkSys" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "LinkSys")),
                Scope::Namespaced,
            ),
            _ => {
                return Err(ConfWriterError::InternalError(format!(
                    "Unknown resource kind: {}",
                    kind
                )))
            }
        };
        Ok((ar, scope))
    }

    /// Create a dynamic API for the given kind and namespace (no Discovery call)
    ///
    /// # Arguments
    /// * `kind` - Resource kind
    /// * `namespace` - For namespaced resources: Some(ns) = specific namespace, None = default namespace
    fn dynamic_api(&self, kind: &str, namespace: Option<&str>) -> Result<Api<DynamicObject>, ConfWriterError> {
        let (ar, scope) = self.resolve_api_resource(kind)?;

        let api: Api<DynamicObject> = match scope {
            Scope::Namespaced => {
                let ns = namespace.unwrap_or("default");
                Api::namespaced_with(self.client.clone(), ns, &ar)
            }
            Scope::Cluster => Api::all_with(self.client.clone(), &ar),
        };

        Ok(api)
    }

    /// Create a dynamic API that lists resources across ALL namespaces (no Discovery call)
    ///
    /// For cluster-scoped resources, behaves the same as `dynamic_api`.
    /// For namespaced resources, uses `Api::all_with()` to list across all namespaces.
    fn dynamic_api_all_namespaces(&self, kind: &str) -> Result<Api<DynamicObject>, ConfWriterError> {
        let (ar, _scope) = self.resolve_api_resource(kind)?;
        // Always use Api::all_with to list across all namespaces
        Ok(Api::all_with(self.client.clone(), &ar))
    }

    /// Map kube::Error to ConfWriterError with appropriate error types
    fn map_kube_error(&self, e: kube::Error) -> ConfWriterError {
        match e {
            kube::Error::Api(ref ae) => match ae.code {
                400 => ConfWriterError::ValidationError(ae.message.clone()),
                403 => ConfWriterError::PermissionDenied(ae.message.clone()),
                404 => ConfWriterError::NotFound(ae.message.clone()),
                409 => ConfWriterError::AlreadyExists(ae.message.clone()),
                422 => ConfWriterError::ValidationError(ae.message.clone()),
                _ => ConfWriterError::KubeError(format!("K8s API error ({}): {}", ae.code, ae.message)),
            },
            _ => ConfWriterError::KubeError(e.to_string()),
        }
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
        let api = self.dynamic_api(kind, namespace)?;

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

    async fn create_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let api = self.dynamic_api(kind, namespace)?;

        // Parse content as DynamicObject
        let obj: DynamicObject = serde_yaml::from_str(&content)
            .map_err(|e| ConfWriterError::ParseError(format!("Failed to parse YAML: {}", e)))?;

        // Use K8s API create (fails if resource already exists with 409)
        api.create(&Default::default(), &obj)
            .await
            .map_err(|e| self.map_kube_error(e))?;

        tracing::info!(
            component = "kubernetes_writer",
            event = "resource_created",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource created via K8s API"
        );

        Ok(())
    }

    async fn update_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let api = self.dynamic_api(kind, namespace)?;

        // Parse content as DynamicObject
        let mut obj: DynamicObject = serde_yaml::from_str(&content)
            .map_err(|e| ConfWriterError::ParseError(format!("Failed to parse YAML: {}", e)))?;

        // Get current resource to obtain resourceVersion (required for replace)
        let current = api.get(name).await.map_err(|e| self.map_kube_error(e))?;

        // Set resourceVersion from current object (required for optimistic concurrency)
        if let Some(rv) = current.metadata.resource_version {
            obj.metadata.resource_version = Some(rv);
        }

        // Use K8s API replace (fails if resource doesn't exist with 404)
        api.replace(name, &Default::default(), &obj)
            .await
            .map_err(|e| self.map_kube_error(e))?;

        tracing::info!(
            component = "kubernetes_writer",
            event = "resource_updated",
            kind = kind,
            namespace = ?namespace,
            name = name,
            "Resource updated via K8s API"
        );

        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfWriterError> {
        let api = self.dynamic_api(kind, namespace)?;

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
        let api = self.dynamic_api(kind, namespace)?;

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

        Ok(ListResult { items, continue_token })
    }

    async fn get_list_by_kind(&self, kind: &str, opts: Option<ListOptions>) -> Result<ListResult, ConfWriterError> {
        // Use dynamic_api_all_namespaces to list across ALL namespaces
        let api = self.dynamic_api_all_namespaces(kind)?;

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
            .map(|obj| {
                let content = serde_yaml::to_string(&obj).unwrap_or_else(|e| {
                    tracing::error!(
                        component = "kubernetes_writer",
                        kind = kind,
                        name = ?obj.metadata.name,
                        error = %e,
                        "Failed to serialize object to YAML"
                    );
                    String::new()
                });
                ConfEntry {
                    kind: kind.to_string(),
                    namespace: obj.metadata.namespace.clone(),
                    name: obj.metadata.name.clone().unwrap_or_default(),
                    content,
                }
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
        let api = self.dynamic_api(kind, Some(namespace))?;

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
            .map(|obj| {
                let content = serde_yaml::to_string(&obj).unwrap_or_else(|e| {
                    tracing::error!(
                        component = "kubernetes_writer",
                        kind = kind,
                        namespace = namespace,
                        name = ?obj.metadata.name,
                        error = %e,
                        "Failed to serialize object to YAML"
                    );
                    String::new()
                });
                ConfEntry {
                    kind: kind.to_string(),
                    namespace: Some(namespace.to_string()),
                    name: obj.metadata.name.clone().unwrap_or_default(),
                    content,
                }
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
