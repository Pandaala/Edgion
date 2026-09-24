//! KubernetesStorage implementation
//!
//! Implements the CenterApi trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use super::version_detection::{
    probe_gateway_class_version, probe_gateway_version, probe_http_route_version, probe_reference_grant_version,
    GatewayClassVersion, GatewayVersion, HTTPRouteVersion, ReferenceGrantVersion,
};
use crate::conf_mgr::conf_center::common::{
    ensure_expected_resource_version_value, merge_operator_update_values, validate_backend_tls_policy_api_version,
    validate_tls_route_api_version,
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
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Kubernetes API based configuration writer
///
/// Implements ConfWriter by calling K8s API server.
/// Uses dynamic client to support any resource type.
pub struct KubernetesStorage {
    client: Client,
    allowed_namespaces: BTreeSet<String>,
    reference_grant_version: Arc<OnceCell<ReferenceGrantVersion>>,
    gateway_class_version: Arc<OnceCell<GatewayClassVersion>>,
    gateway_version: Arc<OnceCell<GatewayVersion>>,
    http_route_version: Arc<OnceCell<HTTPRouteVersion>>,
}

impl KubernetesStorage {
    /// Create a new KubernetesStorage with default client
    pub async fn new(allowed_namespaces: Vec<String>) -> Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self {
            client,
            allowed_namespaces: allowed_namespaces.into_iter().collect(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        })
    }

    /// Share the selected transport with the center writer and all controller epochs.
    pub(super) fn gateway_selection(&self) -> Arc<OnceCell<GatewayVersion>> {
        self.gateway_version.clone()
    }

    pub(super) async fn selected_gateway_version(&self) -> Result<GatewayVersion> {
        self.gateway_version
            .get_or_try_init(|| async {
                probe_gateway_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Gateway CRD not found (expected v1 or v1beta1)"))
            })
            .await
            .copied()
    }

    pub(super) fn http_route_selection(&self) -> Arc<OnceCell<HTTPRouteVersion>> {
        self.http_route_version.clone()
    }

    pub(super) async fn selected_http_route_version(&self) -> Result<HTTPRouteVersion> {
        self.http_route_version
            .get_or_try_init(|| async {
                probe_http_route_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("HTTPRoute CRD not found (expected v1 or v1beta1)"))
            })
            .await
            .copied()
    }

    pub(super) fn gateway_class_selection(&self) -> Arc<OnceCell<GatewayClassVersion>> {
        self.gateway_class_version.clone()
    }

    pub(super) async fn selected_gateway_class_version(&self) -> Result<GatewayClassVersion> {
        self.gateway_class_version
            .get_or_try_init(|| async {
                probe_gateway_class_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("GatewayClass CRD not found (expected v1 or v1beta1)"))
            })
            .await
            .copied()
    }

    pub(super) fn selection(&self) -> Arc<OnceCell<ReferenceGrantVersion>> {
        self.reference_grant_version.clone()
    }

    pub(super) async fn selected_reference_grant_version(&self) -> Result<ReferenceGrantVersion> {
        self.reference_grant_version
            .get_or_try_init(|| async {
                probe_reference_grant_version(&self.client)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("ReferenceGrant CRD not found (expected v1 or v1beta1)"))
            })
            .await
            .copied()
    }

    pub(super) async fn new_with_selection(
        allowed_namespaces: Vec<String>,
        selection: Arc<OnceCell<ReferenceGrantVersion>>,
        gateway_class_selection: Arc<OnceCell<GatewayClassVersion>>,
        gateway_selection: Arc<OnceCell<GatewayVersion>>,
        http_route_selection: Arc<OnceCell<HTTPRouteVersion>>,
    ) -> Result<Self> {
        let mut storage = Self::new(allowed_namespaces).await?;
        storage.reference_grant_version = selection;
        storage.gateway_class_version = gateway_class_selection;
        storage.gateway_version = gateway_selection;
        storage.http_route_version = http_route_selection;
        Ok(storage)
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        }
    }

    /// Resolve ApiResource and Scope for a resource kind.
    async fn resolve_api_resource(&self, kind: &str) -> Result<(ApiResource, Scope), ConfWriterError> {
        fn api_resource(group: &str, version: &str, kind: &str, plural: &str) -> ApiResource {
            ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(group, version, kind), plural)
        }

        // Static mapping of resource kinds to their ApiResource and Scope
        // This avoids expensive Discovery calls on every API operation
        let (ar, scope) =
            match kind {
                // Gateway API resources (namespaced)
                "Gateway" => {
                    let version = self.selected_gateway_version().await.map_err(|e| {
                        ConfWriterError::InternalError(format!("Failed to select Gateway version: {e}"))
                    })?;
                    let version = match version {
                        GatewayVersion::V1 => "v1",
                        GatewayVersion::V1Beta1 => "v1beta1",
                    };
                    (
                        ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", version, "Gateway")),
                        Scope::Namespaced,
                    )
                }
                "GatewayClass" => {
                    let version = self.selected_gateway_class_version().await.map_err(|e| {
                        ConfWriterError::InternalError(format!("Failed to select GatewayClass version: {e}"))
                    })?;
                    let version = match version {
                        GatewayClassVersion::V1 => "v1",
                        GatewayClassVersion::V1Beta1 => "v1beta1",
                    };
                    (
                        ApiResource::from_gvk(&GroupVersionKind::gvk(
                            "gateway.networking.k8s.io",
                            version,
                            "GatewayClass",
                        )),
                        Scope::Cluster,
                    )
                }
                "HTTPRoute" => {
                    let version = self.selected_http_route_version().await.map_err(|e| {
                        ConfWriterError::InternalError(format!("Failed to select HTTPRoute version: {e}"))
                    })?;
                    let version = match version {
                        HTTPRouteVersion::V1 => "v1",
                        HTTPRouteVersion::V1Beta1 => "v1beta1",
                    };
                    (
                        ApiResource::from_gvk(&GroupVersionKind::gvk(
                            "gateway.networking.k8s.io",
                            version,
                            "HTTPRoute",
                        )),
                        Scope::Namespaced,
                    )
                }
                "GRPCRoute" => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "GRPCRoute")),
                    Scope::Namespaced,
                ),
                "TCPRoute" => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "TCPRoute")),
                    Scope::Namespaced,
                ),
                "UDPRoute" => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "UDPRoute")),
                    Scope::Namespaced,
                ),
                "TLSRoute" => (
                    ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "TLSRoute")),
                    Scope::Namespaced,
                ),
                "ReferenceGrant" => {
                    let version = self.selected_reference_grant_version().await.map_err(|e| {
                        ConfWriterError::InternalError(format!("Failed to select ReferenceGrant version: {e}"))
                    })?;
                    let version = match version {
                        ReferenceGrantVersion::V1 => "v1",
                        ReferenceGrantVersion::V1Beta1 => "v1beta1",
                    };
                    (
                        ApiResource::from_gvk(&GroupVersionKind::gvk(
                            "gateway.networking.k8s.io",
                            version,
                            "ReferenceGrant",
                        )),
                        Scope::Namespaced,
                    )
                }
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
        } else if kind == "TLSRoute" {
            validate_tls_route_api_version(value.get("apiVersion").and_then(Value::as_str))
                .map_err(ConfWriterError::ValidationError)?;
        }

        if kind == "Gateway" {
            let canonical: edgion_resources::Gateway =
                crate::conf_mgr::conf_center::common::parse_resource_content(content).map_err(|source| {
                    ConfWriterError::ParseError {
                        context: "Failed to parse Gateway source",
                        source: source.into(),
                    }
                })?;
            value = serde_json::to_value(canonical).map_err(|source| ConfWriterError::ParseError {
                context: "Failed to serialize canonical Gateway",
                source: source.into(),
            })?;
            // The canonical type also carries controller-only conf_sync fields.
            // Kubernetes CRUD sends only operator fields from the source wire.
            if let Some(spec) = value.get_mut("spec").and_then(Value::as_object_mut) {
                for field in [
                    "currentStatus",
                    "resolvedInboundProxyProtocol",
                    "resolvedAttachmentProof",
                ] {
                    spec.remove(field);
                }
                if let Some(backend) = spec
                    .get_mut("tls")
                    .and_then(|tls| tls.get_mut("backend"))
                    .and_then(Value::as_object_mut)
                {
                    backend.remove("resolvedClientCertificate");
                }
                if let Some(listeners) = spec.get_mut("listeners").and_then(Value::as_array_mut) {
                    for listener in listeners {
                        if let Some(tls) = listener.get_mut("tls").and_then(Value::as_object_mut) {
                            for field in [
                                "resolvedCertificateRefs",
                                "frontendValidation",
                                "resolvedFrontendCaRefs",
                                "frontendMatcherEligibility",
                            ] {
                                tls.remove(field);
                            }
                        }
                    }
                }
            }
            project_gateway_api_version(&mut value, &api_resource.kind, &api_resource.api_version);
        }

        if kind == "HTTPRoute" {
            let _canonical: edgion_resources::HTTPRoute =
                crate::conf_mgr::conf_center::common::parse_resource_content(content).map_err(|source| {
                    ConfWriterError::ParseError {
                        context: "Failed to parse HTTPRoute source",
                        source: source.into(),
                    }
                })?;
            // The beta and v1 operator wire fields have the same representation.
            // Keep unknown future Standard fields in an Admin-to-Kubernetes
            // write rather than erasing them during canonical serialization.
            if let Some(spec) = value.get_mut("spec").and_then(Value::as_object_mut) {
                for field in [
                    "currentStatus",
                    "resolvedStatusController",
                    "resolvedHostnames",
                    "resolvedListeners",
                    "invalidRuleIndices",
                    "resolvedRules",
                    "delegationIssues",
                ] {
                    spec.remove(field);
                }
            }
            project_gateway_api_version(&mut value, &api_resource.kind, &api_resource.api_version);
        }

        if kind == "GatewayClass" {
            let canonical: edgion_resources::GatewayClass =
                crate::conf_mgr::conf_center::common::parse_resource_content(content).map_err(|source| {
                    ConfWriterError::ParseError {
                        context: "Failed to parse GatewayClass source",
                        source: source.into(),
                    }
                })?;
            value = serde_json::to_value(canonical).map_err(|source| ConfWriterError::ParseError {
                context: "Failed to serialize canonical GatewayClass",
                source: source.into(),
            })?;
            project_gateway_api_version(&mut value, &api_resource.kind, &api_resource.api_version);
        }

        if kind == "ReferenceGrant" {
            let canonical: edgion_resources::ReferenceGrant =
                crate::conf_mgr::conf_center::common::parse_resource_content(content).map_err(|source| {
                    ConfWriterError::ParseError {
                        context: "Failed to parse ReferenceGrant source",
                        source: source.into(),
                    }
                })?;
            value = serde_json::to_value(canonical).map_err(|source| ConfWriterError::ParseError {
                context: "Failed to serialize canonical ReferenceGrant",
                source: source.into(),
            })?;
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

    #[tokio::test]
    async fn gateway_class_crud_uses_selected_transport_and_projects_beta_source() {
        let storage = KubernetesStorage::new_unconnected_for_test();
        storage.gateway_class_version.set(GatewayClassVersion::V1Beta1).unwrap();
        let (transport, scope) = storage.resolve_api_resource("GatewayClass").await.unwrap();
        assert_eq!(scope, Scope::Cluster);
        assert_eq!(transport.api_version, "gateway.networking.k8s.io/v1beta1");
        let source = json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
            "metadata": {"name": "edge", "resourceVersion": "42"},
            "spec": {"controllerName": "example.com/edge", "description": "edge"},
            "status": {"supportedFeatures": [{"name": "Gateway"}]}
        });
        let object = storage
            .parse_dynamic_object("GatewayClass", &source.to_string(), &transport)
            .unwrap();
        let projected = serde_json::to_value(object).unwrap();
        assert_eq!(projected["apiVersion"], source["apiVersion"]);
        assert_eq!(projected["metadata"], source["metadata"]);
        assert_eq!(projected["spec"], source["spec"]);
        assert_eq!(projected["status"], source["status"]);
        let v1_transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "gateway.networking.k8s.io",
            "v1",
            "GatewayClass",
        ));
        let canonical = storage
            .parse_dynamic_object("GatewayClass", &source.to_string(), &v1_transport)
            .unwrap();
        let canonical = serde_json::to_value(canonical).unwrap();
        assert_eq!(canonical["apiVersion"], "gateway.networking.k8s.io/v1");
        assert_eq!(canonical["metadata"], source["metadata"]);
        assert_eq!(canonical["spec"], source["spec"]);
        assert_eq!(canonical["status"], source["status"]);
    }

    #[tokio::test]
    async fn gateway_class_beta_create_and_get_share_selected_endpoint() {
        let source = json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
            "metadata": {"name": "edge", "resourceVersion": "42"},
            "spec": {"controllerName": "example.com/edge"}
        });
        let captured = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let requests = captured.clone();
        let object = source.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = captured.clone();
            let object = object.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.unwrap();
                let submitted = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                captured
                    .lock()
                    .push((parts.method.clone(), parts.uri.path().into(), submitted.clone()));
                let response = if parts.method == Method::POST {
                    json_response(StatusCode::CREATED, submitted)
                } else {
                    json_response(StatusCode::OK, object)
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        storage.gateway_class_version.set(GatewayClassVersion::V1Beta1).unwrap();
        storage
            .create_one("GatewayClass", None, "edge", source.to_string())
            .await
            .unwrap();
        let fetched: Value =
            serde_yaml::from_str(&storage.get_one("GatewayClass", None, "edge").await.unwrap()).unwrap();
        assert_eq!(fetched["apiVersion"], source["apiVersion"]);
        let requests = requests.lock();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].1, "/apis/gateway.networking.k8s.io/v1beta1/gatewayclasses");
        assert_eq!(requests[0].2["apiVersion"], source["apiVersion"]);
        assert_eq!(
            requests[1].1,
            "/apis/gateway.networking.k8s.io/v1beta1/gatewayclasses/edge"
        );
    }
    #[tokio::test]
    async fn gateway_class_selection_prefers_v1_and_falls_back_only_when_absent() {
        for served_v1 in [true, false] {
            let visited = Arc::new(Mutex::new(Vec::<String>::new()));
            let requests = visited.clone();
            let service = service_fn(move |request: http::Request<Body>| {
                let visited = visited.clone();
                async move {
                    let path = request.uri().path().to_string();
                    visited.lock().push(path.clone());
                    let version = path.rsplit('/').next().unwrap();
                    let response = if version == "v1" && !served_v1 {
                        not_found_response()
                    } else {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "kind": "APIResourceList", "apiVersion": "v1",
                                "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                                "resources": [{"name": "gatewayclasses", "singularName": "gatewayclass",
                                    "namespaced": false, "kind": "GatewayClass", "verbs": ["get", "list", "watch"]}]
                            }),
                        )
                    };
                    Ok::<_, Infallible>(response)
                }
            });
            let storage = KubernetesStorage {
                client: Client::new(service, "default"),
                allowed_namespaces: BTreeSet::new(),
                reference_grant_version: Arc::new(OnceCell::new()),
                gateway_class_version: Arc::new(OnceCell::new()),
                gateway_version: Arc::new(OnceCell::new()),
                http_route_version: Arc::new(OnceCell::new()),
            };
            let selected = storage.selected_gateway_class_version().await.unwrap();
            assert_eq!(
                selected,
                if served_v1 {
                    GatewayClassVersion::V1
                } else {
                    GatewayClassVersion::V1Beta1
                }
            );
            assert_eq!(storage.selected_gateway_class_version().await.unwrap(), selected);
            assert_eq!(requests.lock().len(), if served_v1 { 1 } else { 2 });
        }
    }

    #[tokio::test]
    async fn http_route_selection_prefers_v1_and_falls_back_only_when_absent() {
        for served_v1 in [true, false] {
            let visited = Arc::new(Mutex::new(Vec::<String>::new()));
            let requests = visited.clone();
            let service = service_fn(move |request: http::Request<Body>| {
                let visited = requests.clone();
                async move {
                    let path = request.uri().path().to_string();
                    visited.lock().push(path.clone());
                    let version = path.rsplit('/').next().unwrap();
                    let response = if version == "v1" && !served_v1 {
                        not_found_response()
                    } else {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "kind": "APIResourceList", "apiVersion": "v1",
                                "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                                "resources": [{"name": "httproutes", "singularName": "httproute",
                                    "namespaced": true, "kind": "HTTPRoute", "verbs": ["get", "list", "watch"]}]
                            }),
                        )
                    };
                    Ok::<_, Infallible>(response)
                }
            });
            let storage = KubernetesStorage {
                client: Client::new(service, "default"),
                allowed_namespaces: BTreeSet::new(),
                reference_grant_version: Arc::new(OnceCell::new()),
                gateway_class_version: Arc::new(OnceCell::new()),
                gateway_version: Arc::new(OnceCell::new()),
                http_route_version: Arc::new(OnceCell::new()),
            };
            let selected = storage.selected_http_route_version().await.unwrap();
            assert_eq!(
                selected,
                if served_v1 {
                    HTTPRouteVersion::V1
                } else {
                    HTTPRouteVersion::V1Beta1
                }
            );
            let (wire, _) = storage.resolve_api_resource("HTTPRoute").await.unwrap();
            assert_eq!(
                wire.api_version,
                if served_v1 {
                    "gateway.networking.k8s.io/v1"
                } else {
                    "gateway.networking.k8s.io/v1beta1"
                }
            );
            assert_eq!(visited.lock().len(), if served_v1 { 1 } else { 2 });
        }
    }

    #[tokio::test]
    async fn gateway_crud_projects_beta_source_onto_selected_transport() {
        let storage = KubernetesStorage::new_unconnected_for_test();
        storage.gateway_version.set(GatewayVersion::V1).unwrap();
        let (transport, scope) = storage.resolve_api_resource("Gateway").await.unwrap();
        assert_eq!(scope, Scope::Namespaced);
        assert_eq!(transport.api_version, "gateway.networking.k8s.io/v1");
        let source = json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
            "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "42"},
            "spec": {"gatewayClassName": "edgion", "addresses": [{"value": "192.0.2.1"}],
                "listeners": [{"name": "https", "hostname": "example.com", "port": 443,
                    "protocol": "HTTPS", "tls": {"certificateRefs": [{"name": "server"}]},
                    "allowedRoutes": {"namespaces": {"from": "All"}}}]},
            "status": {"listeners": [{"name": "https", "supportedKinds": [],
                                     "attachedRoutes": 2, "conditions": []}]}
        });
        let projected = serde_json::to_value(
            storage
                .parse_dynamic_object("Gateway", &source.to_string(), &transport)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(projected["apiVersion"], "gateway.networking.k8s.io/v1");
        assert_eq!(projected["metadata"], source["metadata"]);
        assert_eq!(projected["spec"], source["spec"]);
        assert_eq!(projected["status"], source["status"]);
        let beta_transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "gateway.networking.k8s.io",
            "v1beta1",
            "Gateway",
        ));
        let beta = serde_json::to_value(
            storage
                .parse_dynamic_object("Gateway", &source.to_string(), &beta_transport)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(beta, source);
    }

    #[tokio::test]
    async fn http_route_beta_crud_projects_to_selected_wire_version() {
        let storage = KubernetesStorage::new_unconnected_for_test();
        storage.http_route_version.set(HTTPRouteVersion::V1Beta1).unwrap();
        let (transport, scope) = storage.resolve_api_resource("HTTPRoute").await.unwrap();
        assert_eq!(scope, Scope::Namespaced);
        assert_eq!(transport.api_version, "gateway.networking.k8s.io/v1beta1");
        let source = json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
            "metadata": {"name": "route", "namespace": "demo", "resourceVersion": "42"},
            "spec": {"parentRefs": [{"name": "edge"}], "hostnames": ["example.com"],
                "futureStandardField": {"mode": "future"},
                "rules": [{"backendRefs": [{"name": "api", "port": 80}],
                    "filters": [{"type": "ExternalAuth", "externalAuth": {
                        "backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP", "http": {"path": "/check"}
                    }}]}]},
            "status": {"parents": [{"parentRef": {"name": "edge"}, "controllerName": "example.com/other",
                "conditions": []}]}
        });
        let projected = serde_json::to_value(
            storage
                .parse_dynamic_object("HTTPRoute", &source.to_string(), &transport)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(projected["apiVersion"], source["apiVersion"]);
        assert_eq!(projected["metadata"], source["metadata"]);
        assert_eq!(projected["spec"], source["spec"]);
        assert_eq!(projected["status"], source["status"]);

        let v1 = ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "HTTPRoute"));
        let projected = serde_json::to_value(
            storage
                .parse_dynamic_object("HTTPRoute", &source.to_string(), &v1)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(projected["apiVersion"], "gateway.networking.k8s.io/v1");
        assert_eq!(projected["spec"], source["spec"]);
    }

    #[tokio::test]
    async fn http_route_beta_create_and_get_use_selected_namespaced_endpoint() {
        let source = json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
            "metadata": {"name": "route", "namespace": "demo", "resourceVersion": "42"},
            "spec": {"parentRefs": [{"name": "edge"}], "rules": [{"backendRefs": [{"name": "api", "port": 80}]}]}
        });
        let captured = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let requests = captured.clone();
        let object = source.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = requests.clone();
            let object = object.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.unwrap();
                let submitted = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                captured
                    .lock()
                    .push((parts.method.clone(), parts.uri.path().into(), submitted.clone()));
                let response = if parts.method == Method::POST {
                    json_response(StatusCode::CREATED, submitted)
                } else {
                    json_response(StatusCode::OK, object)
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        storage.http_route_version.set(HTTPRouteVersion::V1Beta1).unwrap();
        storage
            .create_one("HTTPRoute", Some("demo"), "route", source.to_string())
            .await
            .unwrap();
        let fetched: Value =
            serde_yaml::from_str(&storage.get_one("HTTPRoute", Some("demo"), "route").await.unwrap()).unwrap();
        assert_eq!(fetched["apiVersion"], source["apiVersion"]);
        let requests = captured.lock();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].1,
            "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/httproutes"
        );
        assert_eq!(requests[0].2["spec"], source["spec"]);
        assert_eq!(
            requests[1].1,
            "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/httproutes/route"
        );
    }

    #[tokio::test]
    async fn gateway_beta_create_and_get_use_selected_namespaced_endpoint() {
        let source = json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
            "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "42"},
            "spec": {"gatewayClassName": "edgion",
                "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
        });
        let captured = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let requests = captured.clone();
        let object = source.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = requests.clone();
            let object = object.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.unwrap();
                let submitted = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                captured
                    .lock()
                    .push((parts.method.clone(), parts.uri.path().into(), submitted.clone()));
                let response = if parts.method == Method::POST {
                    json_response(StatusCode::CREATED, submitted)
                } else {
                    json_response(StatusCode::OK, object)
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        storage.gateway_version.set(GatewayVersion::V1Beta1).unwrap();
        storage
            .create_one("Gateway", Some("demo"), "edge", source.to_string())
            .await
            .unwrap();
        let fetched: Value =
            serde_yaml::from_str(&storage.get_one("Gateway", Some("demo"), "edge").await.unwrap()).unwrap();
        assert_eq!(fetched["apiVersion"], source["apiVersion"]);
        let requests = captured.lock();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].1,
            "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/gateways"
        );
        assert_eq!(requests[0].2["spec"], source["spec"]);
        assert_eq!(
            requests[1].1,
            "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/gateways/edge"
        );
    }

    #[tokio::test]
    async fn gateway_selection_prefers_v1_and_falls_back_only_on_absence() {
        for served_v1 in [true, false] {
            let visited = Arc::new(Mutex::new(Vec::<String>::new()));
            let requests = visited.clone();
            let service = service_fn(move |request: http::Request<Body>| {
                let visited = visited.clone();
                async move {
                    let path = request.uri().path().to_string();
                    visited.lock().push(path.clone());
                    let version = path.rsplit('/').next().unwrap();
                    let response = if version == "v1" && !served_v1 {
                        not_found_response()
                    } else {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "kind": "APIResourceList", "apiVersion": "v1",
                                "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                                "resources": [{"name": "gateways", "singularName": "gateway",
                                    "namespaced": true, "kind": "Gateway", "verbs": ["get", "list", "watch"]}]
                            }),
                        )
                    };
                    Ok::<_, Infallible>(response)
                }
            });
            let storage = KubernetesStorage {
                client: Client::new(service, "default"),
                allowed_namespaces: BTreeSet::new(),
                reference_grant_version: Arc::new(OnceCell::new()),
                gateway_class_version: Arc::new(OnceCell::new()),
                gateway_version: Arc::new(OnceCell::new()),
                http_route_version: Arc::new(OnceCell::new()),
            };
            let selected = storage.selected_gateway_version().await.unwrap();
            assert_eq!(
                selected,
                if served_v1 {
                    GatewayVersion::V1
                } else {
                    GatewayVersion::V1Beta1
                }
            );
            assert_eq!(storage.selected_gateway_version().await.unwrap(), selected);
            assert_eq!(requests.lock().len(), if served_v1 { 1 } else { 2 });
        }
    }

    use http::{Method, Response, StatusCode};
    use kube::api::ObjectMeta;
    use kube::client::Body;
    use kube::core::PartialObjectMeta;
    use parking_lot::Mutex;
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::Arc;
    use tower::service_fn;

    #[tokio::test]
    async fn canonical_crud_resolver_matches_the_typed_source_inventory() {
        use edgion_resources::gateway_api_support::{canonical_sources, GROUP};
        let service = service_fn(|_req: http::Request<Body>| async {
            panic!("canonical CRUD resolution must not perform discovery");
            #[allow(unreachable_code)]
            Ok::<_, Infallible>(Response::new(Body::empty()))
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        storage.gateway_class_version.set(GatewayClassVersion::V1).unwrap();
        storage.gateway_version.set(GatewayVersion::V1).unwrap();
        for source in canonical_sources().filter(|source| source.kind != "ReferenceGrant") {
            let (resolved, scope) = storage.resolve_api_resource(source.kind).await.unwrap();
            assert_eq!(resolved.api_version, format!("{GROUP}/{}", source.version));
            assert_eq!(resolved.kind, source.kind);
            assert_eq!(
                scope,
                if source.kind == "GatewayClass" {
                    Scope::Cluster
                } else {
                    Scope::Namespaced
                }
            );
        }
    }

    #[tokio::test]
    async fn reference_grant_crud_converts_beta_source_to_selected_transport() {
        let storage = KubernetesStorage::new_unconnected_for_test();
        let mut source = canonical_reference_grant_value();
        source["apiVersion"] = json!("gateway.networking.k8s.io/v1beta1");
        let transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "gateway.networking.k8s.io",
            "v1",
            "ReferenceGrant",
        ));
        let object = storage
            .parse_dynamic_object("ReferenceGrant", &source.to_string(), &transport)
            .unwrap();
        let projected = serde_json::to_value(object).unwrap();
        assert_eq!(projected["apiVersion"], "gateway.networking.k8s.io/v1");
        assert_eq!(projected["spec"], source["spec"]);
        let beta_transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "gateway.networking.k8s.io",
            "v1beta1",
            "ReferenceGrant",
        ));
        let beta_object = storage
            .parse_dynamic_object("ReferenceGrant", &source.to_string(), &beta_transport)
            .unwrap();
        assert_eq!(
            serde_json::to_value(beta_object).unwrap()["apiVersion"],
            "gateway.networking.k8s.io/v1beta1"
        );
        for invalid in [
            {
                let mut value = source.clone();
                value["kind"] = json!("ReferenceGrantBeta");
                value
            },
            {
                let mut value = source.clone();
                value["spec"].as_object_mut().unwrap().remove("from");
                value
            },
        ] {
            assert!(storage
                .parse_dynamic_object("ReferenceGrant", &invalid.to_string(), &transport)
                .is_err());
        }
    }

    #[tokio::test]
    async fn reference_grant_crud_resolver_follows_real_discovery() {
        for served in ["v1", "v1beta1"] {
            let service = service_fn(move |request: http::Request<Body>| async move {
                let path = request.uri().path();
                let response = if path == format!("/apis/gateway.networking.k8s.io/{served}") {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "kind": "APIResourceList", "apiVersion": "v1",
                            "groupVersion": format!("gateway.networking.k8s.io/{served}"),
                            "resources": [{"name":"referencegrants", "singularName":"referencegrant",
                                "namespaced":true, "kind":"ReferenceGrant", "verbs":["get","list","watch"]}]
                        }),
                    )
                } else {
                    not_found_response()
                };
                Ok::<_, Infallible>(response)
            });
            let storage = KubernetesStorage {
                client: Client::new(service, "default"),
                allowed_namespaces: BTreeSet::new(),
                reference_grant_version: Arc::new(OnceCell::new()),
                gateway_class_version: Arc::new(OnceCell::new()),
                gateway_version: Arc::new(OnceCell::new()),
                http_route_version: Arc::new(OnceCell::new()),
            };
            let (resolved, scope) = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
            assert_eq!(resolved.api_version, format!("gateway.networking.k8s.io/{served}"));
            assert_eq!(resolved.kind, "ReferenceGrant");
            assert_eq!(scope, Scope::Namespaced);
        }
    }

    #[tokio::test]
    async fn reference_grant_prefers_v1_when_both_versions_are_served() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let probes = Arc::new(AtomicUsize::new(0));
        let count = probes.clone();
        let service = service_fn(move |_request: http::Request<Body>| {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(reference_grant_discovery_response())
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        let (resource, _) = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
        assert_eq!(resource.api_version, "gateway.networking.k8s.io/v1");
        let _ = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
        assert_eq!(probes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reference_grant_selection_is_shared_and_pinned_after_discovery() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let v1_available = Arc::new(AtomicBool::new(false));
        let probes = Arc::new(AtomicUsize::new(0));
        let available = v1_available.clone();
        let count = probes.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let available = available.clone();
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let path = request.uri().path();
                let served = path == "/apis/gateway.networking.k8s.io/v1beta1"
                    || (path == "/apis/gateway.networking.k8s.io/v1" && available.load(Ordering::SeqCst));
                Ok::<_, Infallible>(if served {
                    reference_grant_discovery_response()
                } else {
                    not_found_response()
                })
            }
        });
        let client = Client::new(service, "default");
        let selected = Arc::new(OnceCell::new());
        let make_storage = || KubernetesStorage {
            client: client.clone(),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: selected.clone(),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        let main = make_storage();
        let runtime = make_storage();
        assert_eq!(
            main.selected_reference_grant_version().await.unwrap(),
            ReferenceGrantVersion::V1Beta1
        );
        assert_eq!(probes.load(Ordering::SeqCst), 2);
        v1_available.store(true, Ordering::SeqCst);
        for storage in [&main, &runtime] {
            let (resource, _) = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
            assert_eq!(resource.api_version, "gateway.networking.k8s.io/v1beta1");
        }
        assert_eq!(probes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn shared_reference_grant_selection_pins_main_and_runtime_crud_paths() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let v1_available = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let available = v1_available.clone();
        let captured = requests.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let available = available.clone();
            let captured = captured.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.expect("request body must be readable");
                let value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                let path = parts.uri.path().to_string();
                captured
                    .lock()
                    .push((parts.method.clone(), path.clone(), value.clone()));
                let current = || {
                    let mut resource = canonical_reference_grant_value();
                    resource["apiVersion"] = json!("gateway.networking.k8s.io/v1beta1");
                    resource["metadata"]["resourceVersion"] = json!("42");
                    resource
                };
                let response = if path == "/apis/gateway.networking.k8s.io/v1" {
                    if available.load(Ordering::SeqCst) {
                        reference_grant_discovery_response()
                    } else {
                        not_found_response()
                    }
                } else if path == "/apis/gateway.networking.k8s.io/v1beta1" {
                    reference_grant_discovery_response()
                } else if parts.method == Method::GET && path.ends_with("/referencegrants") {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "gateway.networking.k8s.io/v1beta1",
                            "kind": "ReferenceGrantList",
                            "metadata": {"resourceVersion": "42"},
                            "items": [current()]
                        }),
                    )
                } else if parts.method == Method::GET || parts.method == Method::DELETE {
                    json_response(StatusCode::OK, current())
                } else {
                    json_response(
                        if parts.method == Method::POST {
                            StatusCode::CREATED
                        } else {
                            StatusCode::OK
                        },
                        value,
                    )
                };
                Ok::<_, Infallible>(response)
            }
        });
        let client = Client::new(service, "default");
        let selection = Arc::new(OnceCell::new());
        let storage = || KubernetesStorage {
            client: client.clone(),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: selection.clone(),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        let main = storage();
        let runtime = storage();
        let mut beta_source = canonical_reference_grant_value();
        beta_source["apiVersion"] = json!("gateway.networking.k8s.io/v1beta1");
        main.create_one(
            "ReferenceGrant",
            Some("backend"),
            "allow-route",
            beta_source.to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            main.selected_reference_grant_version().await.unwrap(),
            ReferenceGrantVersion::V1Beta1
        );
        v1_available.store(true, Ordering::SeqCst);
        runtime
            .update_one_if_version(
                "ReferenceGrant",
                Some("backend"),
                "allow-route",
                canonical_reference_grant_value().to_string(),
                Some("42"),
            )
            .await
            .unwrap();
        let fetched = main
            .get_one("ReferenceGrant", Some("backend"), "allow-route")
            .await
            .unwrap();
        assert!(fetched.contains("gateway.networking.k8s.io/v1beta1"));
        let listed = runtime
            .get_list_by_kind_ns("ReferenceGrant", "backend", None)
            .await
            .unwrap();
        assert_eq!(listed.items.len(), 1);
        assert!(listed.items[0].content.contains("gateway.networking.k8s.io/v1beta1"));
        main.delete_one_if_version("ReferenceGrant", Some("backend"), "allow-route", Some("42"))
            .await
            .unwrap();
        let requests = requests.lock();
        assert_eq!(
            requests
                .iter()
                .filter(|(_, path, _)| path == "/apis/gateway.networking.k8s.io/v1")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|(_, path, _)| path == "/apis/gateway.networking.k8s.io/v1beta1")
                .count(),
            1
        );
        let operations: Vec<_> = requests
            .iter()
            .filter(|(_, path, _)| path.contains("/referencegrants"))
            .collect();
        assert_eq!(
            operations
                .iter()
                .map(|(method, _, _)| method)
                .filter(|method| **method == Method::POST)
                .count(),
            1
        );
        assert_eq!(
            operations
                .iter()
                .map(|(method, _, _)| method)
                .filter(|method| **method == Method::PUT)
                .count(),
            1
        );
        assert_eq!(
            operations
                .iter()
                .map(|(method, _, _)| method)
                .filter(|method| **method == Method::DELETE)
                .count(),
            1
        );
        assert!(operations.iter().all(
            |(_, path, _)| path.starts_with("/apis/gateway.networking.k8s.io/v1beta1/") && !path.ends_with("/status")
        ));
        for (method, _, value) in operations {
            if method == Method::POST || method == Method::PUT {
                assert_eq!(value["apiVersion"], "gateway.networking.k8s.io/v1beta1");
                assert_eq!(value["kind"], "ReferenceGrant");
            }
        }
    }

    #[tokio::test]
    async fn transient_reference_grant_discovery_failure_retries_before_selection() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let service = service_fn(move |_request: http::Request<Body>| {
            let count = count.clone();
            async move {
                let attempt = count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(if attempt == 0 {
                    json_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        json!({
                            "kind": "Status", "apiVersion": "v1", "status": "Failure",
                            "reason": "ServiceUnavailable", "message": "temporarily unavailable", "code": 503
                        }),
                    )
                } else {
                    reference_grant_discovery_response()
                })
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        assert!(storage.selected_reference_grant_version().await.is_err());
        assert_eq!(
            storage.selected_reference_grant_version().await.unwrap(),
            ReferenceGrantVersion::V1
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn tls_route_storage_uses_v1_without_discovery_and_rejects_other_versions() {
        let service = service_fn(|_req: http::Request<Body>| async {
            panic!("TLSRoute endpoint selection and raw parsing must not perform discovery");
            #[allow(unreachable_code)]
            Ok::<_, Infallible>(Response::new(Body::empty()))
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        let (api_resource, _) = storage.resolve_api_resource("TLSRoute").await.unwrap();
        assert_eq!(api_resource.api_version, "gateway.networking.k8s.io/v1");
        assert_eq!(api_resource.kind, "TLSRoute");
        for version in [
            None,
            Some(Value::Null),
            Some(json!(42)),
            Some(json!("")),
            Some(json!("gateway.networking.k8s.io/v1alpha3")),
            Some(json!("gateway.networking.k8s.io/v1")),
        ] {
            let valid = version.as_ref().and_then(Value::as_str) == Some("gateway.networking.k8s.io/v1");
            let mut document = json!({"kind": "TLSRoute", "metadata": {"name": "route", "namespace": "default"}, "spec": {"hostnames": ["example.com"]}});
            if let Some(value) = version {
                document["apiVersion"] = value;
            }
            let parsed = storage.parse_dynamic_object("TLSRoute", &document.to_string(), &api_resource);
            if valid {
                assert_eq!(
                    parsed.unwrap().types.unwrap().api_version,
                    "gateway.networking.k8s.io/v1"
                );
            } else {
                assert!(matches!(parsed, Err(ConfWriterError::ValidationError(_))));
            }
        }
    }

    #[tokio::test]
    async fn tls_route_create_persists_v1_and_rejects_legacy_before_http() {
        let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
        let captured = requests.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = captured.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect_bytes().await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                captured
                    .lock()
                    .push((parts.method, parts.uri.path().to_string(), value.clone()));
                Ok::<_, Infallible>(json_response(StatusCode::CREATED, value))
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };
        let mut value = json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha3", "kind": "TLSRoute",
            "metadata": {"name": "route", "namespace": "default"},
            "spec": {"hostnames": ["example.com"]}
        });
        assert!(matches!(
            storage
                .create_one("TLSRoute", Some("default"), "route", value.to_string())
                .await,
            Err(ConfWriterError::ValidationError(_))
        ));
        assert!(requests.lock().is_empty());
        value["apiVersion"] = json!("gateway.networking.k8s.io/v1");
        storage
            .create_one("TLSRoute", Some("default"), "route", value.to_string())
            .await
            .unwrap();
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, Method::POST);
        assert_eq!(
            requests[0].1,
            "/apis/gateway.networking.k8s.io/v1/namespaces/default/tlsroutes"
        );
        assert_eq!(requests[0].2["apiVersion"], "gateway.networking.k8s.io/v1");
    }

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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
        };

        storage
            .delete_one_if_version("ReferenceGrant", Some("backend"), "allow-route", Some("42"))
            .await
            .expect("conditional Kubernetes delete must succeed");

        let requests = requests.lock();
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
                captured.lock().push(request.uri().path().to_string());
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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
        let paths = paths.lock();
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
                    .iter()
                    .filter(|(method, _, _)| *method == Method::PUT)
                    .count();
                captured
                    .lock()
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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

        let requests = requests.lock();
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
                captured.lock().push(method.clone());
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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
        assert_eq!(*requests.lock(), vec![Method::GET, Method::PUT]);
    }

    #[tokio::test]
    async fn unconditional_update_does_not_retry_non_conflict_replace_error() {
        let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let method = request.method().clone();
                captured.lock().push(method.clone());
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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

        assert_eq!(*requests.lock(), vec![Method::GET, Method::PUT]);
    }

    #[tokio::test]
    async fn unconditional_update_does_not_retry_a_persistent_conflict() {
        let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
        let captured = Arc::clone(&requests);
        let service = service_fn(move |request: http::Request<Body>| {
            let captured = Arc::clone(&captured);
            async move {
                let method = request.method().clone();
                captured.lock().push(method.clone());
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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
        let methods = requests.lock();
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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

        let requests = requests.lock();
        assert!(requests.iter().all(|(_, path, _)| !path.ends_with("/status")));
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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

        let requests = requests.lock();
        assert!(requests.iter().all(|(_, path, _)| !path.ends_with("/status")));
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
                captured.lock().push(request.uri().to_string());
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
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
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
        let paths = paths.lock();
        assert_eq!(paths.len(), 1, "RBAC failure must not be treated as v1 absence");
        assert_eq!(paths[0].trim_end_matches('?'), "/apis/gateway.networking.k8s.io/v1");
    }
}
