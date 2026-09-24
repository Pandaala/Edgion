//! KubernetesStorage implementation
//!
//! Implements the CenterApi trait by calling Kubernetes API.
//! Similar to client-go, this allows creating/updating/deleting K8s resources via API.

use super::version_detection::{
    probe_gateway_class_version, probe_gateway_version, probe_http_route_version, probe_legacy_route_version,
    probe_reference_grant_version, GatewayClassVersion, GatewayVersion, HTTPRouteVersion, LegacyRouteVersion,
    ReferenceGrantVersion,
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
    legacy_versions: LegacySelections,
}

#[derive(Default)]
struct LegacySelections {
    grpc_route: OnceCell<Option<LegacyRouteVersion>>,
    tls_route: OnceCell<Option<LegacyRouteVersion>>,
    backend_tls_policy: OnceCell<Option<LegacyRouteVersion>>,
    tcp_route: OnceCell<Option<LegacyRouteVersion>>,
    udp_route: OnceCell<Option<LegacyRouteVersion>>,
}

impl LegacySelections {
    fn cell(&self, kind: &str) -> Option<&OnceCell<Option<LegacyRouteVersion>>> {
        match kind {
            "GRPCRoute" => Some(&self.grpc_route),
            "TLSRoute" => Some(&self.tls_route),
            "BackendTLSPolicy" => Some(&self.backend_tls_policy),
            "TCPRoute" => Some(&self.tcp_route),
            "UDPRoute" => Some(&self.udp_route),
            _ => None,
        }
    }
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
            legacy_versions: LegacySelections::default(),
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

    pub(super) async fn selected_legacy_version(&self, kind: &str) -> Result<Option<LegacyRouteVersion>> {
        let cell = self
            .legacy_versions
            .cell(kind)
            .ok_or_else(|| anyhow::anyhow!("Unsupported Gateway API source kind: {kind}"))?;
        cell.get_or_try_init(|| async { probe_legacy_route_version(&self.client, kind).await })
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
                "GRPCRoute" | "TCPRoute" | "UDPRoute" | "TLSRoute" | "BackendTLSPolicy" => {
                    let version = self
                        .selected_legacy_version(kind)
                        .await
                        .map_err(|error| {
                            ConfWriterError::InternalError(format!("Failed to select {kind} version: {error}"))
                        })?
                        .ok_or_else(|| ConfWriterError::InternalError(format!("{kind} CRD is not installed")))?;
                    (
                        ApiResource::from_gvk(&GroupVersionKind::gvk(
                            "gateway.networking.k8s.io",
                            version.as_str(),
                            kind,
                        )),
                        Scope::Namespaced,
                    )
                }
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

        if matches!(
            kind,
            "GRPCRoute" | "TLSRoute" | "BackendTLSPolicy" | "TCPRoute" | "UDPRoute"
        ) {
            let resource_kind = edgion_resources::ResourceKind::from_kind_name(kind)
                .ok_or_else(|| ConfWriterError::ValidationError(format!("Unknown Gateway API kind: {kind}")))?;
            let raw = serde_yaml::to_value(&value).map_err(|source| ConfWriterError::ParseError {
                context: "Failed to decode Gateway API source",
                source: source.into(),
            })?;
            crate::conf_mgr::conf_center::common::validate_gateway_api_source(resource_kind, &raw)
                .map_err(ConfWriterError::ValidationError)?;
            if matches!(kind, "TCPRoute" | "UDPRoute")
                && value["spec"]["rules"].as_array().is_some_and(|rules| rules.len() > 1)
            {
                return Err(ConfWriterError::ValidationError(format!(
                    "{kind} v1 supports at most one rule"
                )));
            }
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
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/storage_test.rs"]
mod tests;
