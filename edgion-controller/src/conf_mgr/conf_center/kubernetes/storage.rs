//! KubernetesStorage implementation
//!
//! Implements the CenterApi trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use super::version_detection::{
    detect_reference_grant_version, probe_backend_tls_policy_version, probe_tls_route_version, BackendTLSPolicyVersion,
    ReferenceGrantVersion, TLSRouteVersion,
};
use crate::conf_mgr::conf_center::traits::{
    CenterApi, ConfEntry, ConfWriterError, ListOptions, ListResult, ResourceKey, ResourceKeyMeta, LAST_APPLIED_KEY,
};
use anyhow::Result;
use async_trait::async_trait;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::discovery::Scope;
use kube::{Api, Client};
use serde_json::json;

/// Kubernetes API based configuration writer
///
/// Implements ConfWriter by calling K8s API server.
/// Uses dynamic client to support any resource type.
pub struct KubernetesStorage {
    client: Client,
}

impl KubernetesStorage {
    /// Create a new KubernetesStorage with default client
    pub async fn new() -> Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self { client })
    }

    /// Create a new KubernetesStorage with existing client
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// Get the Kubernetes client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Resolve ApiResource and Scope for a resource kind.
    async fn resolve_api_resource(&self, kind: &str) -> Result<(ApiResource, Scope), ConfWriterError> {
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
            "TLSRoute" => match probe_tls_route_version(&self.client)
                .await
                .map_err(|e| ConfWriterError::InternalError(format!("Failed to detect TLSRoute version: {}", e)))?
            {
                Some(TLSRouteVersion::V1) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "TLSRoute")),
                    Scope::Namespaced,
                ),
                Some(TLSRouteVersion::V1Alpha3) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1alpha3",
                        "TLSRoute",
                    )),
                    Scope::Namespaced,
                ),
                None => {
                    return Err(ConfWriterError::NotFound(
                        "TLSRoute CRD not found (expected v1 or v1alpha3)".to_string(),
                    ))
                }
            },
            "ReferenceGrant" => match detect_reference_grant_version(&self.client).await {
                Some(ReferenceGrantVersion::V1Beta1) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1beta1",
                        "ReferenceGrant",
                    )),
                    Scope::Namespaced,
                ),
                _ => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1",
                        "ReferenceGrant",
                    )),
                    Scope::Namespaced,
                ),
            },
            "BackendTLSPolicy" => match probe_backend_tls_policy_version(&self.client).await.map_err(|e| {
                ConfWriterError::InternalError(format!("Failed to detect BackendTLSPolicy version: {}", e))
            })? {
                Some(BackendTLSPolicyVersion::V1) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1",
                        "BackendTLSPolicy",
                    )),
                    Scope::Namespaced,
                ),
                Some(BackendTLSPolicyVersion::V1Alpha3) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1alpha3",
                        "BackendTLSPolicy",
                    )),
                    Scope::Namespaced,
                ),
                None => {
                    return Err(ConfWriterError::NotFound(
                        "BackendTLSPolicy CRD not found (expected v1 or v1alpha3)".to_string(),
                    ))
                }
            },
            // Core resources
            "Secret" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Secret")),
                Scope::Namespaced,
            ),
            "Service" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Service")),
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
            "EdgionData" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "EdgionData")),
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

    async fn backend_tls_policy_api_version(&self) -> Result<Option<&'static str>, ConfWriterError> {
        let version = probe_backend_tls_policy_version(&self.client)
            .await
            .map_err(|e| ConfWriterError::InternalError(format!("Failed to detect BackendTLSPolicy version: {}", e)))?;

        Ok(match version {
            Some(BackendTLSPolicyVersion::V1) => Some("gateway.networking.k8s.io/v1"),
            Some(BackendTLSPolicyVersion::V1Alpha3) => Some("gateway.networking.k8s.io/v1alpha3"),
            None => None,
        })
    }

    async fn tls_route_api_version(&self) -> Result<Option<&'static str>, ConfWriterError> {
        let version = probe_tls_route_version(&self.client)
            .await
            .map_err(|e| ConfWriterError::InternalError(format!("Failed to detect TLSRoute version: {}", e)))?;

        Ok(match version {
            Some(TLSRouteVersion::V1) => Some("gateway.networking.k8s.io/v1"),
            Some(TLSRouteVersion::V1Alpha3) => Some("gateway.networking.k8s.io/v1alpha3"),
            None => None,
        })
    }

    /// Create a dynamic API for the given kind and namespace (no Discovery call)
    ///
    /// # Arguments
    /// * `kind` - Resource kind
    /// * `namespace` - For namespaced resources: Some(ns) = specific namespace, None = default namespace
    async fn dynamic_api(&self, kind: &str, namespace: Option<&str>) -> Result<Api<DynamicObject>, ConfWriterError> {
        let (ar, scope) = self.resolve_api_resource(kind).await?;

        let api: Api<DynamicObject> = match scope {
            Scope::Namespaced => {
                let ns = namespace.unwrap_or("default");
                Api::namespaced_with(self.client.clone(), ns, &ar)
            }
            Scope::Cluster => Api::all_with(self.client.clone(), &ar),
        };

        Ok(api)
    }

    async fn parse_dynamic_object(&self, kind: &str, content: &str) -> Result<DynamicObject, ConfWriterError> {
        let mut value: serde_json::Value = serde_yaml::from_str(content).map_err(|e| ConfWriterError::ParseError {
            context: "Failed to parse YAML",
            source: e.into(),
        })?;

        if kind == "BackendTLSPolicy" {
            let Some(api_version) = self.backend_tls_policy_api_version().await? else {
                return Err(ConfWriterError::NotFound(
                    "BackendTLSPolicy CRD not found (expected v1 or v1alpha3)".to_string(),
                ));
            };

            if let Some(obj) = value.as_object_mut() {
                obj.insert("apiVersion".to_string(), json!(api_version));
                obj.insert("kind".to_string(), json!("BackendTLSPolicy"));
            }
        }

        if kind == "TLSRoute" {
            let Some(api_version) = self.tls_route_api_version().await? else {
                return Err(ConfWriterError::NotFound(
                    "TLSRoute CRD not found (expected v1 or v1alpha3)".to_string(),
                ));
            };

            if let Some(obj) = value.as_object_mut() {
                obj.insert("apiVersion".to_string(), json!(api_version));
                obj.insert("kind".to_string(), json!("TLSRoute"));
            }
        }

        serde_json::from_value(value).map_err(|e| ConfWriterError::ParseError {
            context: "Failed to deserialize JSON",
            source: e.into(),
        })
    }

    /// Create a dynamic API that lists resources across ALL namespaces (no Discovery call)
    ///
    /// For cluster-scoped resources, behaves the same as `dynamic_api`.
    /// For namespaced resources, uses `Api::all_with()` to list across all namespaces.
    async fn dynamic_api_all_namespaces(&self, kind: &str) -> Result<Api<DynamicObject>, ConfWriterError> {
        let (ar, _scope) = self.resolve_api_resource(kind).await?;
        // Always use Api::all_with to list across all namespaces
        Ok(Api::all_with(self.client.clone(), &ar))
    }

    /// Map kube::Error to ConfWriterError with appropriate error types.
    /// Well-known API error codes are mapped to domain variants (String payload).
    /// All other errors are wrapped in `KubeError` with the original source preserved.
    fn map_kube_error(&self, e: kube::Error) -> ConfWriterError {
        map_kube_error(e)
    }
}

/// Map kube::Error to ConfWriterError with appropriate error types.
///
/// Well-known API error codes are mapped to domain variants (String payload).
/// All other errors are wrapped in `KubeError` with the original source preserved.
///
/// Extracted as a free function so it can be unit-tested without a live kube client.
fn map_kube_error(e: kube::Error) -> ConfWriterError {
    if let kube::Error::Api(ae) = &e {
        match ae.code {
            400 => return ConfWriterError::ValidationError(ae.message.clone()),
            403 => return ConfWriterError::PermissionDenied(ae.message.clone()),
            404 => return ConfWriterError::NotFound(ae.message.clone()),
            409 => return ConfWriterError::AlreadyExists(ae.message.clone()),
            410 => return ConfWriterError::StalePagination(ae.message.clone()),
            422 => return ConfWriterError::ValidationError(ae.message.clone()),
            _ => {}
        }
    }
    ConfWriterError::KubeError(Box::new(e))
}

/// Project a kube `PartialObjectMeta` into a transport-friendly `ResourceKey`.
///
/// Drops the `last-applied-configuration` annotation (multi-KB blob the K8s
/// apiserver attaches on `kubectl apply -f`). Other annotations and labels
/// pass through unchanged.
fn project_partial_meta_to_key<K>(kind: &str, part: kube::core::PartialObjectMeta<K>) -> ResourceKey {
    let api_version = part.types.as_ref().map(|t| t.api_version.clone()).unwrap_or_default();

    let meta = part.metadata;
    let mut annotations: std::collections::BTreeMap<String, String> =
        meta.annotations.unwrap_or_default().into_iter().collect();
    annotations.remove(LAST_APPLIED_KEY);

    let labels: std::collections::BTreeMap<String, String> = meta.labels.unwrap_or_default().into_iter().collect();

    let creation_timestamp = meta.creation_timestamp.map(|t| t.0);

    ResourceKey {
        kind: kind.to_string(),
        api_version,
        metadata: ResourceKeyMeta {
            namespace: meta.namespace,
            name: meta.name.unwrap_or_default(),
            creation_timestamp,
            labels,
            annotations,
            resource_version: meta.resource_version,
        },
    }
}

#[async_trait]
impl CenterApi for KubernetesStorage {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfWriterError> {
        let api = self.dynamic_api(kind, namespace).await?;

        // Parse content as DynamicObject
        let obj = self.parse_dynamic_object(kind, &content).await?;

        // Use server-side apply
        let params = PatchParams::apply("edgion-controller").force();
        api.patch(name, &params, &Patch::Apply(&obj))
            .await
            .map_err(|e| self.map_kube_error(e))?;

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
        let api = self.dynamic_api(kind, namespace).await?;

        // Parse content as DynamicObject
        let obj = self.parse_dynamic_object(kind, &content).await?;

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
        let api = self.dynamic_api(kind, namespace).await?;

        // Parse content as DynamicObject
        let mut obj = self.parse_dynamic_object(kind, &content).await?;

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
        let api = self.dynamic_api(kind, namespace).await?;

        let obj = api.get(name).await.map_err(|e| self.map_kube_error(e))?;

        let content = serde_yaml::to_string(&obj).map_err(|e| ConfWriterError::ParseError {
            context: "Failed to serialize to YAML",
            source: e.into(),
        })?;

        Ok(content)
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfWriterError> {
        let api = self.dynamic_api(kind, namespace).await?;

        let params = DeleteParams::default();
        api.delete(name, &params).await.map_err(|e| self.map_kube_error(e))?;

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
            "EndpointSlice",
            "EdgionTls",
            "EdgionGatewayConfig",
            "EdgionPlugins",
            "EdgionStreamPlugins",
            "BackendTLSPolicy",
            "EdgionData",
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
        let api = self.dynamic_api_all_namespaces(kind).await?;

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

        let list = api.list(&lp).await.map_err(|e| self.map_kube_error(e))?;

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
        let api = self.dynamic_api(kind, Some(namespace)).await?;

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

        let list = api.list(&lp).await.map_err(|e| self.map_kube_error(e))?;

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

    async fn list_keys_by_kind(
        &self,
        kind: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        let api = self.dynamic_api_all_namespaces(kind).await?;

        let mut lp = ListParams::default();
        if let Some(ref options) = opts {
            if options.limit > 0 {
                lp = lp.limit(options.limit);
            }
            if let Some(ref token) = options.continue_token {
                lp = lp.continue_token(token);
            }
        }

        let list = api.list_metadata(&lp).await.map_err(|e| self.map_kube_error(e))?;

        let items: Vec<ResourceKey> = list
            .items
            .into_iter()
            .map(|part| project_partial_meta_to_key(kind, part))
            .collect();

        tracing::debug!(
            component = "kubernetes_writer",
            kind = kind,
            count = items.len(),
            "Listed metadata-only keys by kind (all namespaces)"
        );

        Ok(ListResult {
            items,
            continue_token: list.metadata.continue_,
        })
    }

    async fn list_keys_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
        opts: Option<ListOptions>,
    ) -> Result<ListResult<ResourceKey>, ConfWriterError> {
        let api = self.dynamic_api(kind, Some(namespace)).await?;

        let mut lp = ListParams::default();
        if let Some(ref options) = opts {
            if options.limit > 0 {
                lp = lp.limit(options.limit);
            }
            if let Some(ref token) = options.continue_token {
                lp = lp.continue_token(token);
            }
        }

        let list = api.list_metadata(&lp).await.map_err(|e| self.map_kube_error(e))?;

        let items: Vec<ResourceKey> = list
            .items
            .into_iter()
            .map(|part| project_partial_meta_to_key(kind, part))
            .collect();

        tracing::debug!(
            component = "kubernetes_writer",
            kind = kind,
            namespace = namespace,
            count = items.len(),
            "Listed metadata-only keys by kind+ns"
        );

        Ok(ListResult {
            items,
            continue_token: list.metadata.continue_,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::api::ObjectMeta;
    use kube::core::PartialObjectMeta;
    use std::collections::BTreeMap;

    #[test]
    fn project_partial_meta_strips_last_applied_and_preserves_others() {
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "kubectl.kubernetes.io/last-applied-configuration".into(),
            "{\"big\":\"yaml\"}".repeat(200),
        );
        annotations.insert("edgion.io/enable-http".into(), "true".into());
        annotations.insert("kubernetes.io/ingress.class".into(), "edgion".into());

        let part: PartialObjectMeta<kube::api::DynamicObject> = PartialObjectMeta {
            types: Some(kube::core::TypeMeta {
                api_version: "gateway.networking.k8s.io/v1".into(),
                kind: "HTTPRoute".into(),
            }),
            metadata: ObjectMeta {
                name: Some("my-route".into()),
                namespace: Some("default".into()),
                resource_version: Some("42".into()),
                annotations: Some(annotations),
                labels: Some({
                    let mut l = BTreeMap::new();
                    l.insert("app".into(), "demo".into());
                    l
                }),
                ..Default::default()
            },
            _phantom: std::marker::PhantomData,
        };

        let key = project_partial_meta_to_key("HTTPRoute", part);
        assert_eq!(key.kind, "HTTPRoute");
        assert_eq!(key.metadata.name, "my-route");
        assert_eq!(key.metadata.namespace.as_deref(), Some("default"));
        assert_eq!(key.metadata.resource_version.as_deref(), Some("42"));
        assert!(!key
            .metadata
            .annotations
            .contains_key("kubectl.kubernetes.io/last-applied-configuration"));
        assert_eq!(
            key.metadata
                .annotations
                .get("edgion.io/enable-http")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            key.metadata
                .annotations
                .get("kubernetes.io/ingress.class")
                .map(String::as_str),
            Some("edgion")
        );
        assert_eq!(key.metadata.labels.get("app").map(String::as_str), Some("demo"));
    }

    #[test]
    fn map_kube_error_410_to_stale_pagination() {
        let ae = kube::core::ErrorResponse {
            status: "Failure".into(),
            message: "The provided continue parameter is too old".into(),
            reason: "Expired".into(),
            code: 410,
        };
        let err = kube::Error::Api(ae);
        let mapped = map_kube_error(err);
        assert!(
            matches!(mapped, ConfWriterError::StalePagination(_)),
            "expected StalePagination, got: {:?}",
            mapped
        );
        if let ConfWriterError::StalePagination(msg) = mapped {
            assert!(
                msg.contains("continue parameter is too old"),
                "unexpected message: {msg}"
            );
        }
    }
}
