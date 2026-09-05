//! KubernetesStorage implementation
//!
//! Implements the CenterApi trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use super::version_detection::{
    probe_reference_grant_version, probe_tls_route_version, ReferenceGrantVersion, TLSRouteVersion,
};
use crate::conf_mgr::conf_center::common::{
    ensure_expected_resource_version_value, merge_operator_update_values, validate_backend_tls_policy_api_version,
};
use crate::conf_mgr::conf_center::traits::{
    CenterApi, ConfEntry, ConfWriterError, ListOptions, ListResult, ResourceKey, ResourceKeyMeta, LAST_APPLIED_KEY,
};
use anyhow::Result;
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, Preconditions};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::discovery::Scope;
use kube::{Api, Client};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Kubernetes API based configuration writer
///
/// Implements ConfWriter by calling K8s API server.
/// Uses dynamic client to support any resource type.
pub struct KubernetesStorage {
    client: Client,
    allowed_namespaces: BTreeSet<String>,
}

impl KubernetesStorage {
    /// Create a new KubernetesStorage with default client
    pub async fn new(allowed_namespaces: Vec<String>) -> Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self {
            client,
            allowed_namespaces: allowed_namespaces.into_iter().collect(),
        })
    }

    /// Test-only constructor backed by an in-process stub transport.
    ///
    /// Lets a unit test build a `KubernetesCenter` without a cluster and
    /// without a TLS stack. The stub service answers every request with an
    /// empty 200; no test using this issues a request. Must be called from
    /// within a Tokio runtime (`Client::new` spawns a buffer worker).
    #[cfg(test)]
    pub(super) fn new_unconnected_for_test() -> Self {
        let service = tower::service_fn(|_req: http::Request<kube::client::Body>| async {
            Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::empty()))
        });
        Self {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        }
    }

    /// Resolve ApiResource and Scope for a resource kind.
    async fn resolve_api_resource(&self, kind: &str) -> Result<(ApiResource, Scope), ConfWriterError> {
        fn api_resource(group: &str, version: &str, kind: &str, plural: &str) -> ApiResource {
            ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(group, version, kind), plural)
        }

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
            "ReferenceGrant" => match probe_reference_grant_version(&self.client).await.map_err(|e| {
                ConfWriterError::InternalError(format!("Failed to detect ReferenceGrant version: {}", e))
            })? {
                Some(ReferenceGrantVersion::V1Beta1) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1beta1",
                        "ReferenceGrant",
                    )),
                    Scope::Namespaced,
                ),
                Some(ReferenceGrantVersion::V1) => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "gateway.networking.k8s.io",
                        "v1",
                        "ReferenceGrant",
                    )),
                    Scope::Namespaced,
                ),
                None => {
                    return Err(ConfWriterError::NotFound(
                        "ReferenceGrant CRD not found (expected v1 or v1beta1)".to_string(),
                    ))
                }
            },
            "BackendTLSPolicy" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk(
                    "gateway.networking.k8s.io",
                    "v1",
                    "BackendTLSPolicy",
                )),
                Scope::Namespaced,
            ),
            // Core resources
            "Secret" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Secret")),
                Scope::Namespaced,
            ),
            "ConfigMap" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "ConfigMap")),
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
            // Edgion CRDs.
            //
            // These CRDs declare non-conventional plurals (e.g. EdgionPlugins ->
            // "edgionplugins", LinkSys -> "linksys"). `api_resource` pins the
            // CRD-declared plural so admin list/get hits the correct resource;
            // the conventional plural inferred by `from_gvk` ("edgionpluginses" /
            // "linksyses") would not match the CRD, and the K8s API server would
            // reject the request (403 from RBAC, since the ClusterRole grants
            // only the real plural).
            "EdgionTls" => (
                api_resource("edgion.io", "v1", "EdgionTls", "edgiontls"),
                Scope::Namespaced,
            ),
            "EdgionGatewayConfig" => (
                api_resource(
                    "edgion.io",
                    "v1alpha1",
                    "EdgionGatewayConfig",
                    "edgiongatewayclassconfigs",
                ),
                Scope::Cluster,
            ),
            "EdgionPlugins" => (
                api_resource("edgion.io", "v1", "EdgionPlugins", "edgionplugins"),
                Scope::Namespaced,
            ),
            "EdgionStreamPlugins" => (
                api_resource("edgion.io", "v1", "EdgionStreamPlugins", "edgionstreamplugins"),
                Scope::Namespaced,
            ),
            "EdgionConfigData" => (
                api_resource("edgion.io", "v1", "EdgionConfigData", "edgionconfigdata"),
                Scope::Namespaced,
            ),
            "LinkSys" => (api_resource("edgion.io", "v1", "LinkSys", "linksys"), Scope::Namespaced),
            "EdgionBackend" => (
                api_resource("edgion.io", "v1", "EdgionBackend", "edgionbackends"),
                Scope::Namespaced,
            ),
            // EdgionAcme: the conventional plural ("edgionacmes") matches the CRD,
            // so `from_gvk` resolves correctly without a plural override.
            "EdgionAcme" => (
                ApiResource::from_gvk(&GroupVersionKind::gvk("edgion.io", "v1", "EdgionAcme")),
                Scope::Namespaced,
            ),
            // Non-conventional plural: naive pluralization would yield
            // "edgionbackendtrafficpolicys", so an explicit plural is required.
            "EdgionBackendTrafficPolicy" => (
                api_resource(
                    "edgion.io",
                    "v1",
                    "EdgionBackendTrafficPolicy",
                    "edgionbackendtrafficpolicies",
                ),
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

    fn namespace_allowed(&self, namespace: Option<&str>) -> bool {
        self.allowed_namespaces.is_empty()
            || namespace.is_some_and(|namespace| self.allowed_namespaces.contains(namespace))
    }

    /// Create a dynamic API for the given kind and namespace (no Discovery call)
    ///
    /// # Arguments
    /// * `kind` - Resource kind
    /// * `namespace` - For namespaced resources: Some(ns) = specific namespace, None = default namespace
    async fn dynamic_api(&self, kind: &str, namespace: Option<&str>) -> Result<Api<DynamicObject>, ConfWriterError> {
        self.dynamic_api_and_resource(kind, namespace).await.map(|(api, _)| api)
    }

    /// Resolve the served GVK once and build the matching dynamic API. Mutation
    /// paths retain the returned ApiResource and use that exact apiVersion for
    /// request projection, preventing a second probe from choosing a different URL/GVK.
    async fn dynamic_api_and_resource(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Result<(Api<DynamicObject>, ApiResource), ConfWriterError> {
        let (ar, scope) = self.resolve_api_resource(kind).await?;

        let api: Api<DynamicObject> = match scope {
            Scope::Namespaced => {
                let ns = namespace.unwrap_or("default");
                if !self.namespace_allowed(Some(ns)) {
                    return Err(ConfWriterError::PermissionDenied(format!(
                        "Namespace {ns} is outside conf_center.kubernetes.watch_namespaces"
                    )));
                }
                Api::namespaced_with(self.client.clone(), ns, &ar)
            }
            Scope::Cluster => Api::all_with(self.client.clone(), &ar),
        };

        Ok((api, ar))
    }

    fn parse_dynamic_object(
        &self,
        kind: &str,
        content: &str,
        api_resource: &ApiResource,
    ) -> Result<DynamicObject, ConfWriterError> {
        let mut value: serde_json::Value = serde_yaml::from_str(content).map_err(|e| ConfWriterError::ParseError {
            context: "Failed to parse YAML",
            source: e.into(),
        })?;

        if kind == "BackendTLSPolicy" {
            validate_backend_tls_policy_api_version(value.get("apiVersion").and_then(Value::as_str))
                .map_err(ConfWriterError::ValidationError)?;
        }

        if matches!(kind, "TLSRoute" | "ReferenceGrant") {
            project_gateway_api_version(&mut value, &api_resource.kind, &api_resource.api_version);
        }

        serde_json::from_value(value).map_err(|e| ConfWriterError::ParseError {
            context: "Failed to deserialize JSON",
            source: e.into(),
        })
    }

    /// Map kube::Error to ConfWriterError with appropriate error types.
    /// Well-known API error codes are mapped to domain variants (String payload).
    /// All other errors are wrapped in `KubeError` with the original source preserved.
    fn map_kube_error(&self, e: kube::Error) -> ConfWriterError {
        map_kube_error(e)
    }
}

/// Project a canonical Gateway API document to the version served by the
/// target cluster. ReferenceGrant v1 and v1beta1 have the same wire schema,
/// so projection is an explicit GVK rewrite rather than a lossy re-shaping.
fn project_gateway_api_version(value: &mut Value, kind: &str, api_version: &str) {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("apiVersion".to_string(), json!(api_version));
        obj.insert("kind".to_string(), json!(kind));
    }
}

fn merge_dynamic_operator_update(
    current: DynamicObject,
    requested: DynamicObject,
) -> Result<DynamicObject, ConfWriterError> {
    let current = serde_json::to_value(current).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to serialize current Kubernetes resource for update",
        source: source.into(),
    })?;
    let requested = serde_json::to_value(requested).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to serialize updated Kubernetes resource",
        source: source.into(),
    })?;
    let merged = merge_operator_update_values(current, requested)?;
    serde_json::from_value(merged).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to deserialize merged Kubernetes resource update",
        source: source.into(),
    })
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
            // Kubernetes uses 409 for create collisions and optimistic-
            // concurrency failures. Both remain HTTP 409, while the caller's
            // method determines the operator-facing guidance.
            409 => return ConfWriterError::Conflict(ae.message.clone()),
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

const NAMESPACE_PAGE_TOKEN_PREFIX: &str = "edgion-ns-v2:";

#[derive(Deserialize, Serialize)]
struct NamespacePageToken {
    kind: String,
    namespaces: Vec<String>,
    resource_version: String,
    offset: usize,
}

fn paginate_namespaced<T>(
    items: Vec<T>,
    opts: Option<ListOptions>,
    kind: &str,
    allowed_namespaces: &BTreeSet<String>,
    resource_version: Option<&str>,
) -> Result<ListResult<T>, ConfWriterError> {
    let Some(options) = opts else {
        return Ok(ListResult {
            items,
            continue_token: None,
        });
    };
    let namespaces = allowed_namespaces.iter().cloned().collect::<Vec<_>>();
    let resource_version = resource_version.unwrap_or_default();
    let offset = match options.continue_token {
        Some(token) => {
            let encoded = token.strip_prefix(NAMESPACE_PAGE_TOKEN_PREFIX).ok_or_else(|| {
                ConfWriterError::StalePagination("Invalid namespace-scoped continue token".to_string())
            })?;
            let decoded = URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| ConfWriterError::StalePagination("Invalid namespace-scoped continue token".to_string()))?;
            let token: NamespacePageToken = serde_json::from_slice(&decoded)
                .map_err(|_| ConfWriterError::StalePagination("Invalid namespace-scoped continue token".to_string()))?;
            if token.kind != kind
                || token.namespaces != namespaces
                || token.resource_version != resource_version
                || token.offset > items.len()
            {
                return Err(ConfWriterError::StalePagination(
                    "Namespace-scoped result set changed while paginating".to_string(),
                ));
            }
            token.offset
        }
        None => 0,
    };
    if options.limit == 0 {
        return Ok(ListResult {
            items: items.into_iter().skip(offset).collect(),
            continue_token: None,
        });
    }
    let end = offset.saturating_add(options.limit as usize).min(items.len());
    let continue_token = (end < items.len()).then(|| {
        let token = NamespacePageToken {
            kind: kind.to_string(),
            namespaces,
            resource_version: resource_version.to_string(),
            offset: end,
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&token).expect("page token serialization cannot fail"));
        format!("{NAMESPACE_PAGE_TOKEN_PREFIX}{encoded}")
    });
    Ok(ListResult {
        items: items.into_iter().skip(offset).take(end - offset).collect(),
        continue_token,
    })
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
        let (api, api_resource) = self.dynamic_api_and_resource(kind, namespace).await?;

        let obj = self.parse_dynamic_object(kind, &content, &api_resource)?;

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
        let (api, api_resource) = self.dynamic_api_and_resource(kind, namespace).await?;

        let obj = self.parse_dynamic_object(kind, &content, &api_resource)?;

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

    async fn update_one_if_version(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
        expected_resource_version: Option<&str>,
    ) -> Result<(), ConfWriterError> {
        let (api, api_resource) = self.dynamic_api_and_resource(kind, namespace).await?;

        let requested = self.parse_dynamic_object(kind, &content, &api_resource)?;

        // Get current resource to obtain resourceVersion (required for replace)
        let current = api.get(name).await.map_err(|e| self.map_kube_error(e))?;

        let current_value = serde_json::to_value(&current).map_err(|source| ConfWriterError::ParseError {
            context: "Failed to serialize current Kubernetes resourceVersion",
            source: source.into(),
        })?;
        ensure_expected_resource_version_value(&current_value, expected_resource_version)?;

        // Preserve the current status and Kubernetes-owned metadata, while
        // replacing labels, annotations, and operator fields from the request.
        // The merge starts from the request, so resolved fields in current.spec
        // are deliberately not restored.
        let obj = merge_dynamic_operator_update(current, requested)?;

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

    async fn delete_one_if_version(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        expected_resource_version: Option<&str>,
    ) -> Result<(), ConfWriterError> {
        let api = self.dynamic_api(kind, namespace).await?;

        let params = DeleteParams {
            preconditions: expected_resource_version.map(|version| Preconditions {
                resource_version: Some(version.to_string()),
                uid: None,
            }),
            ..DeleteParams::default()
        };
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
            "ConfigMap",
            "Service",
            "EndpointSlice",
            "EdgionTls",
            "EdgionGatewayConfig",
            "EdgionPlugins",
            "EdgionStreamPlugins",
            "BackendTLSPolicy",
            "EdgionConfigData",
            "LinkSys",
            "EdgionAcme",
            "EdgionBackendTrafficPolicy",
            "EdgionBackend",
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
        let (api_resource, scope) = self.resolve_api_resource(kind).await?;
        let restrict_namespaces = scope == Scope::Namespaced && !self.allowed_namespaces.is_empty();

        if restrict_namespaces {
            let mut items = Vec::new();
            let mut snapshot = Vec::new();
            for namespace in &self.allowed_namespaces {
                let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &api_resource);
                let list = api
                    .list(&ListParams::default())
                    .await
                    .map_err(|e| self.map_kube_error(e))?;
                snapshot.push(format!(
                    "{namespace}={}",
                    list.metadata.resource_version.as_deref().unwrap_or_default()
                ));
                items.extend(list.items.into_iter().map(|obj| {
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
                }));
            }
            items.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
            return paginate_namespaced(items, opts, kind, &self.allowed_namespaces, Some(&snapshot.join(",")));
        }

        let api: Api<DynamicObject> = Api::all_with(self.client.clone(), &api_resource);

        let mut lp = ListParams::default();
        if let Some(ref options) = opts {
            if options.limit > 0 {
                lp = lp.limit(options.limit);
            }
            if let Some(ref token) = options.continue_token {
                lp = lp.continue_token(token);
            }
        }

        let list = api.list(&lp).await.map_err(|e| self.map_kube_error(e))?;

        let items: Vec<ConfEntry> = list
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
        let (api_resource, scope) = self.resolve_api_resource(kind).await?;
        let restrict_namespaces = scope == Scope::Namespaced && !self.allowed_namespaces.is_empty();

        if restrict_namespaces {
            let mut items = Vec::new();
            let mut snapshot = Vec::new();
            for namespace in &self.allowed_namespaces {
                let api: Api<DynamicObject> = Api::namespaced_with(self.client.clone(), namespace, &api_resource);
                let list = api
                    .list_metadata(&ListParams::default())
                    .await
                    .map_err(|e| self.map_kube_error(e))?;
                snapshot.push(format!(
                    "{namespace}={}",
                    list.metadata.resource_version.as_deref().unwrap_or_default()
                ));
                items.extend(
                    list.items
                        .into_iter()
                        .map(|part| project_partial_meta_to_key(kind, part)),
                );
            }
            items.sort_by(|a, b| {
                (&a.metadata.namespace, &a.metadata.name).cmp(&(&b.metadata.namespace, &b.metadata.name))
            });
            return paginate_namespaced(items, opts, kind, &self.allowed_namespaces, Some(&snapshot.join(",")));
        }

        let api: Api<DynamicObject> = Api::all_with(self.client.clone(), &api_resource);

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
    use http::{Method, Response, StatusCode};
    use kube::api::ObjectMeta;
    use kube::client::Body;
    use kube::core::PartialObjectMeta;
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};
    use tower::service_fn;

    fn json_response(status: StatusCode, value: Value) -> Response<Body> {
        Response::builder()
            .status(status)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&value).expect("response JSON must serialize"),
            ))
            .expect("response must build")
    }

    fn not_found_response() -> Response<Body> {
        json_response(
            StatusCode::NOT_FOUND,
            json!({
                "apiVersion": "v1",
                "kind": "Status",
                "status": "Failure",
                "message": "the server could not find the requested resource",
                "reason": "NotFound",
                "code": 404
            }),
        )
    }

    fn conflict_response(message: &str) -> Response<Body> {
        json_response(
            StatusCode::CONFLICT,
            json!({
                "apiVersion": "v1",
                "kind": "Status",
                "status": "Failure",
                "message": message,
                "reason": "Conflict",
                "code": 409
            }),
        )
    }

    fn config_map_value(resource_version: Option<&str>, status_state: Option<&str>, data_value: &str) -> Value {
        let mut value = json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {
                "name": "hot-config",
                "namespace": "default",
                "uid": "uid-1"
            },
            "data": {"value": data_value}
        });
        if let Some(resource_version) = resource_version {
            value["metadata"]["resourceVersion"] = json!(resource_version);
        }
        if let Some(status_state) = status_state {
            value["status"] = json!({"state": status_state});
        }
        value
    }

    fn reference_grant_discovery_response() -> Response<Body> {
        json_response(
            StatusCode::OK,
            json!({
                "apiVersion": "v1",
                "groupVersion": "gateway.networking.k8s.io/v1beta1",
                "kind": "APIResourceList",
                "resources": [{
                    "name": "referencegrants",
                    "singularName": "referencegrant",
                    "namespaced": true,
                    "kind": "ReferenceGrant",
                    "verbs": ["create", "delete", "get", "list", "patch", "update", "watch"]
                }]
            }),
        )
    }

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

    #[tokio::test]
    async fn delete_sends_kubernetes_resource_version_precondition() {
        let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.expect("request body must be readable");
                let value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).expect("request body must be JSON")
                };
                captured
                    .lock()
                    .unwrap()
                    .push((parts.method.clone(), parts.uri.to_string(), value));
                let response = if parts.uri.path() == "/apis/gateway.networking.k8s.io/v1" {
                    not_found_response()
                } else if parts.method == Method::GET {
                    reference_grant_discovery_response()
                } else {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "Status",
                            "status": "Success",
                            "code": 200
                        }),
                    )
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        storage
            .delete_one_if_version("ReferenceGrant", Some("backend"), "allow-route", Some("42"))
            .await
            .expect("conditional Kubernetes delete must succeed");

        let requests = requests.lock().unwrap();
        let delete = requests
            .iter()
            .find(|(method, _, _)| method == Method::DELETE)
            .expect("delete request must be captured");
        assert_eq!(delete.2["preconditions"]["resourceVersion"], "42");
    }

    #[tokio::test]
    async fn configmap_key_listing_is_limited_to_watched_namespaces_and_paginates_locally() {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&paths);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push(request.uri().path().to_string());
                Ok::<_, Infallible>(json_response(
                    StatusCode::OK,
                    json!({
                        "apiVersion": "meta.k8s.io/v1",
                        "kind": "PartialObjectMetadataList",
                        "metadata": {"resourceVersion": "7"},
                        "items": [
                            {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "a-2", "namespace": "allowed"}},
                            {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "a-1", "namespace": "allowed"}}
                        ]
                    }),
                ))
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::from(["allowed".to_string()]),
        };

        let first = storage
            .list_keys_by_kind(
                "ConfigMap",
                Some(ListOptions {
                    limit: 1,
                    continue_token: None,
                }),
            )
            .await
            .expect("first page must succeed");
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].metadata.name, "a-1");
        let token = first.continue_token.expect("first page must continue");

        let second = storage
            .list_keys_by_kind(
                "ConfigMap",
                Some(ListOptions {
                    limit: 1,
                    continue_token: Some(token),
                }),
            )
            .await
            .expect("second page must succeed");
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].metadata.name, "a-2");
        assert!(second.continue_token.is_none());
        let paths = paths.lock().expect("request capture lock must not be poisoned");
        assert_eq!(
            paths.as_slice(),
            [
                "/api/v1/namespaces/allowed/configmaps",
                "/api/v1/namespaces/allowed/configmaps"
            ]
        );
    }

    #[test]
    fn namespace_pagination_rejects_foreign_tokens() {
        let error = paginate_namespaced(
            vec![1],
            Some(ListOptions {
                limit: 1,
                continue_token: Some("kubernetes-token".to_string()),
            }),
            "ConfigMap",
            &BTreeSet::from(["allowed".to_string()]),
            Some("7"),
        )
        .expect_err("foreign tokens must be rejected");
        assert!(matches!(error, ConfWriterError::StalePagination(_)));
    }

    #[test]
    fn namespace_pagination_token_is_bound_to_query_and_snapshot() {
        let namespaces = BTreeSet::from(["allowed".to_string()]);
        let first = paginate_namespaced(
            vec![1, 2],
            Some(ListOptions {
                limit: 1,
                continue_token: None,
            }),
            "ConfigMap",
            &namespaces,
            Some("7"),
        )
        .expect("first page must succeed");
        let token = first.continue_token.expect("first page must continue");

        for (kind, resource_version) in [("Secret", "7"), ("ConfigMap", "8")] {
            let error = paginate_namespaced(
                vec![1, 2],
                Some(ListOptions {
                    limit: 1,
                    continue_token: Some(token.clone()),
                }),
                kind,
                &namespaces,
                Some(resource_version),
            )
            .expect_err("token reuse across a query or snapshot must fail");
            assert!(matches!(error, ConfWriterError::StalePagination(_)));
        }
    }

    fn canonical_reference_grant_value() -> Value {
        json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "ReferenceGrant",
            "metadata": {
                "name": "allow-route",
                "namespace": "backend"
            },
            "spec": {
                "from": [{
                    "group": "gateway.networking.k8s.io",
                    "kind": "HTTPRoute",
                    "namespace": "frontend"
                }],
                "to": [{"group": "", "kind": "Service"}]
            }
        })
    }

    #[test]
    fn backend_tls_policy_api_version_accepts_only_v1() {
        let canonical = json!({"apiVersion": "gateway.networking.k8s.io/v1"});
        assert!(validate_backend_tls_policy_api_version(canonical.get("apiVersion").and_then(Value::as_str)).is_ok());

        for value in [json!({"apiVersion": "gateway.networking.k8s.io/v1alpha3"}), json!({})] {
            let error =
                validate_backend_tls_policy_api_version(value.get("apiVersion").and_then(Value::as_str)).unwrap_err();
            assert!(error.contains("BackendTLSPolicy apiVersion must be gateway.networking.k8s.io/v1"));
        }
    }

    #[test]
    fn create_projects_canonical_reference_grant_to_detected_v1beta1_gvk() {
        let mut value = canonical_reference_grant_value();
        project_gateway_api_version(&mut value, "ReferenceGrant", "gateway.networking.k8s.io/v1beta1");

        let object: DynamicObject = serde_json::from_value(value).expect("projected object must deserialize");
        let types = object.types.expect("projected object must retain type metadata");
        assert_eq!(types.api_version, "gateway.networking.k8s.io/v1beta1");
        assert_eq!(types.kind, "ReferenceGrant");
        assert_eq!(object.data["spec"]["from"][0]["namespace"], "frontend");
    }

    #[test]
    fn update_projection_keeps_v1beta1_and_preserves_server_fields() {
        let mut requested = canonical_reference_grant_value();
        requested["metadata"]["labels"] = json!({"edited": "true"});
        requested["spec"]["to"] = json!([{"group": "", "kind": "Secret"}]);
        project_gateway_api_version(&mut requested, "ReferenceGrant", "gateway.networking.k8s.io/v1beta1");

        let current: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1",
            "kind": "ReferenceGrant",
            "metadata": {
                "name": "allow-route",
                "namespace": "backend",
                "uid": "uid-1",
                "resourceVersion": "42",
                "generation": 3,
                "ownerReferences": [{
                    "apiVersion": "v1",
                    "kind": "ConfigMap",
                    "name": "owner",
                    "uid": "owner-uid"
                }],
                "finalizers": ["edgion.io/finalizer"]
            },
            "spec": {
                "from": [{
                    "group": "gateway.networking.k8s.io",
                    "kind": "HTTPRoute",
                    "namespace": "old"
                }],
                "to": [{"group": "", "kind": "Service"}],
                "runtimeOnly": "must-not-return"
            },
            "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
        }))
        .expect("current object must deserialize");
        let requested: DynamicObject = serde_json::from_value(requested).expect("projected request must deserialize");

        let merged = merge_dynamic_operator_update(current, requested).expect("update merge must succeed");
        let merged = serde_json::to_value(merged).expect("merged object must serialize");
        assert_eq!(merged["apiVersion"], "gateway.networking.k8s.io/v1beta1");
        assert_eq!(merged["metadata"]["resourceVersion"], "42");
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["metadata"]["ownerReferences"][0]["name"], "owner");
        assert_eq!(merged["metadata"]["finalizers"][0], "edgion.io/finalizer");
        assert_eq!(merged["metadata"]["labels"]["edited"], "true");
        assert_eq!(merged["spec"]["to"][0]["kind"], "Secret");
        assert!(merged["spec"].get("runtimeOnly").is_none());
        assert_eq!(merged["status"]["conditions"][0]["type"], "Accepted");
    }

    #[test]
    fn stale_expected_resource_version_stops_kubernetes_replace() {
        let current = json!({"metadata": {"resourceVersion": "42"}});
        let error = ensure_expected_resource_version_value(&current, Some("41"))
            .expect_err("stale Kubernetes update must conflict before replace");
        assert!(matches!(error, ConfWriterError::Conflict(_)));
    }

    #[tokio::test]
    async fn unconditional_update_surfaces_replace_conflict_without_server_retry() {
        let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.expect("request body must be readable");
                let value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).expect("request body must be JSON")
                };
                let prior_puts = captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .iter()
                    .filter(|(method, _, _)| *method == Method::PUT)
                    .count();
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push((parts.method.clone(), parts.uri.to_string(), value.clone()));

                let response = match parts.method {
                    Method::GET if prior_puts == 0 => {
                        json_response(StatusCode::OK, config_map_value(Some("42"), Some("old"), "old"))
                    }
                    Method::GET => json_response(StatusCode::OK, config_map_value(Some("43"), Some("latest"), "old")),
                    Method::PUT if prior_puts == 0 => conflict_response("object was modified by status update"),
                    Method::PUT => json_response(StatusCode::OK, value),
                    _ => panic!("unexpected request: {} {}", parts.method, parts.uri),
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        let error = storage
            .update_one_if_version(
                "ConfigMap",
                Some("default"),
                "hot-config",
                serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
                None,
            )
            .await
            .expect_err("the Admin API must surface the backend conflict to its caller");

        assert!(matches!(error, ConfWriterError::Conflict(_)));

        let requests = requests.lock().expect("request capture lock must not be poisoned");
        let methods = requests.iter().map(|(method, _, _)| method.clone()).collect::<Vec<_>>();
        assert_eq!(methods, vec![Method::GET, Method::PUT]);
        let puts = requests
            .iter()
            .filter(|(method, _, _)| *method == Method::PUT)
            .map(|(_, _, body)| body)
            .collect::<Vec<_>>();
        assert_eq!(puts[0]["metadata"]["resourceVersion"], "42");
        assert_eq!(puts[0]["status"]["state"], "old");
        assert_eq!(puts[0]["data"]["value"], "requested");
    }

    #[tokio::test]
    async fn conditional_update_does_not_retry_replace_conflict() {
        let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let method = request.method().clone();
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push(method.clone());
                let response = if method == Method::GET {
                    json_response(StatusCode::OK, config_map_value(Some("42"), Some("old"), "old"))
                } else if method == Method::PUT {
                    conflict_response("object was modified by status update")
                } else {
                    panic!("unexpected request method: {method}")
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        let error = storage
            .update_one_if_version(
                "ConfigMap",
                Some("default"),
                "hot-config",
                serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
                Some("42"),
            )
            .await
            .expect_err("explicit CAS must surface a replace conflict");

        assert!(matches!(error, ConfWriterError::Conflict(_)));
        assert_eq!(
            *requests.lock().expect("request capture lock must not be poisoned"),
            vec![Method::GET, Method::PUT]
        );
    }

    #[tokio::test]
    async fn unconditional_update_does_not_retry_non_conflict_replace_error() {
        let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let method = request.method().clone();
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push(method.clone());
                let response = if method == Method::GET {
                    json_response(StatusCode::OK, config_map_value(Some("42"), Some("old"), "old"))
                } else if method == Method::PUT {
                    json_response(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        json!({
                            "apiVersion": "v1",
                            "kind": "Status",
                            "status": "Failure",
                            "message": "invalid update",
                            "reason": "Invalid",
                            "code": 422
                        }),
                    )
                } else {
                    panic!("unexpected request method: {method}")
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        storage
            .update_one_if_version(
                "ConfigMap",
                Some("default"),
                "hot-config",
                serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
                None,
            )
            .await
            .expect_err("a non-conflict replace error must be returned without retry");

        assert_eq!(
            *requests.lock().expect("request capture lock must not be poisoned"),
            vec![Method::GET, Method::PUT]
        );
    }

    #[tokio::test]
    async fn unconditional_update_does_not_retry_a_persistent_conflict() {
        let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let method = request.method().clone();
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push(method.clone());
                let response = if method == Method::GET {
                    json_response(StatusCode::OK, config_map_value(Some("42"), Some("busy"), "old"))
                } else if method == Method::PUT {
                    conflict_response("object keeps changing")
                } else {
                    panic!("unexpected request method: {method}")
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        let error = storage
            .update_one_if_version(
                "ConfigMap",
                Some("default"),
                "hot-config",
                serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
                None,
            )
            .await
            .expect_err("the first backend conflict must be surfaced");

        assert!(matches!(error, ConfWriterError::Conflict(_)));
        let methods = requests.lock().expect("request capture lock must not be poisoned");
        assert_eq!(methods.as_slice(), &[Method::GET, Method::PUT]);
    }

    #[tokio::test]
    async fn create_reuses_one_reference_grant_resolution_for_url_and_body_gvk() {
        let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.expect("request body must be readable");
                let value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).expect("request body must be JSON")
                };
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push((parts.method.clone(), parts.uri.to_string(), value.clone()));

                let response = if parts.uri.path() == "/apis/gateway.networking.k8s.io/v1" {
                    not_found_response()
                } else if parts.method == Method::GET && parts.uri.path() == "/apis/gateway.networking.k8s.io/v1beta1" {
                    reference_grant_discovery_response()
                } else {
                    json_response(StatusCode::CREATED, value)
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        storage
            .create_one(
                "ReferenceGrant",
                Some("backend"),
                "allow-route",
                serde_yaml::to_string(&canonical_reference_grant_value()).expect("fixture must serialize"),
            )
            .await
            .expect("v1beta1 create must succeed");

        let requests = requests.lock().expect("request capture lock must not be poisoned");
        assert_eq!(requests.len(), 3, "one v1 probe, one v1beta1 probe, and one create");
        let create = requests
            .iter()
            .find(|(method, _, _)| method == Method::POST)
            .expect("create request must be captured");
        assert_eq!(
            create.1.trim_end_matches('?'),
            "/apis/gateway.networking.k8s.io/v1beta1/namespaces/backend/referencegrants"
        );
        assert_eq!(create.2["apiVersion"], "gateway.networking.k8s.io/v1beta1");
        assert_eq!(create.2["kind"], "ReferenceGrant");
    }

    #[tokio::test]
    async fn update_reuses_one_reference_grant_resolution_for_url_and_body_gvk() {
        let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.expect("request body must be readable");
                let value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).expect("request body must be JSON")
                };
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push((parts.method.clone(), parts.uri.to_string(), value.clone()));

                let path = parts.uri.path();
                let response = if path == "/apis/gateway.networking.k8s.io/v1" {
                    not_found_response()
                } else if parts.method == Method::GET && path == "/apis/gateway.networking.k8s.io/v1beta1" {
                    reference_grant_discovery_response()
                } else if parts.method == Method::GET {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "gateway.networking.k8s.io/v1beta1",
                            "kind": "ReferenceGrant",
                            "metadata": {
                                "name": "allow-route",
                                "namespace": "backend",
                                "uid": "uid-1",
                                "resourceVersion": "42",
                                "finalizers": ["edgion.io/finalizer"]
                            },
                            "spec": canonical_reference_grant_value()["spec"].clone(),
                            "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
                        }),
                    )
                } else {
                    json_response(StatusCode::OK, value)
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };
        let mut requested = canonical_reference_grant_value();
        requested["metadata"]["labels"] = json!({"edited": "true"});

        storage
            .update_one_if_version(
                "ReferenceGrant",
                Some("backend"),
                "allow-route",
                serde_yaml::to_string(&requested).expect("fixture must serialize"),
                Some("42"),
            )
            .await
            .expect("v1beta1 update must succeed");

        let requests = requests.lock().expect("request capture lock must not be poisoned");
        assert_eq!(
            requests.len(),
            4,
            "one v1 probe, one v1beta1 probe, one get, and one replace"
        );
        let replace = requests
            .iter()
            .find(|(method, _, _)| method == Method::PUT)
            .expect("replace request must be captured");
        assert_eq!(
            replace.1.trim_end_matches('?'),
            "/apis/gateway.networking.k8s.io/v1beta1/namespaces/backend/referencegrants/allow-route"
        );
        assert_eq!(replace.2["apiVersion"], "gateway.networking.k8s.io/v1beta1");
        assert_eq!(replace.2["metadata"]["resourceVersion"], "42");
        assert_eq!(replace.2["metadata"]["finalizers"][0], "edgion.io/finalizer");
        assert_eq!(replace.2["status"]["conditions"][0]["type"], "Accepted");
    }

    #[tokio::test]
    async fn reference_grant_probe_propagates_forbidden_without_fallback() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&paths);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                captured
                    .lock()
                    .expect("request capture lock must not be poisoned")
                    .push(request.uri().to_string());
                Ok::<_, Infallible>(json_response(
                    StatusCode::FORBIDDEN,
                    json!({
                        "apiVersion": "v1",
                        "kind": "Status",
                        "status": "Failure",
                        "message": "referencegrants is forbidden",
                        "reason": "Forbidden",
                        "code": 403
                    }),
                ))
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
        };

        let err = storage
            .create_one(
                "ReferenceGrant",
                Some("backend"),
                "allow-route",
                serde_yaml::to_string(&canonical_reference_grant_value()).expect("fixture must serialize"),
            )
            .await
            .expect_err("forbidden probe must fail");
        let message = match err {
            ConfWriterError::InternalError(message) => message,
            other => panic!("expected internal probe error, got {other:?}"),
        };
        assert!(message.to_ascii_lowercase().contains("forbidden"));
        let paths = paths.lock().expect("request capture lock must not be poisoned");
        assert_eq!(paths.len(), 1, "RBAC failure must not be treated as v1 absence");
        assert_eq!(paths[0].trim_end_matches('?'), "/apis/gateway.networking.k8s.io/v1");
    }
}
