//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources with Gateway API standard status management.

use std::collections::BTreeMap;

use k8s_openapi::ByteString;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::backend_tls_policy_ref::{
    get_backend_tls_policy_target_manager, get_sibling_policies, is_policy_winner, make_target_conflict_key,
    remove_policy_timestamp, set_policy_timestamp,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::configmap_utils::get_configmap;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key, get_secret,
    update_condition, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use crate::types::prelude_resources::{BackendTLSPolicy, BackendTLSPolicyAlpha3};
use crate::types::resources::backend_tls_policy::{
    BackendTLSPolicyCACertificateRef, BackendTLSPolicyStatus, PolicyAncestorStatus,
};
use crate::types::resources::common::{is_core_api_group, ParentReference};
use crate::types::ResourceKind;

/// BackendTLSPolicy handler
pub struct BackendTlsPolicyHandler {
    controller_name: String,
}

impl BackendTlsPolicyHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }
}

impl Default for BackendTlsPolicyHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

impl BackendTlsPolicyHandler {
    fn validate_policy(&self, btp: &BackendTLSPolicy) -> Vec<String> {
        let mut errors = Vec::new();

        let has_ca_refs = btp
            .spec
            .validation
            .ca_certificate_refs
            .as_ref()
            .is_some_and(|refs| !refs.is_empty());
        let has_well_known = btp.spec.validation.well_known_ca_certificates.is_some();

        if has_ca_refs && has_well_known {
            errors.push("caCertificateRefs and wellKnownCACertificates are mutually exclusive".to_string());
        }

        if !has_ca_refs && !has_well_known {
            errors.push("must specify either caCertificateRefs or wellKnownCACertificates".to_string());
        }

        errors
    }

    async fn parse_policy(&self, mut btp: BackendTLSPolicy, ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            btp.metadata.namespace.clone(),
            btp.metadata.name.clone().unwrap_or_default(),
        );

        // Gateway API v1.5 restricts BackendTLSPolicy to a single targetRef.
        // Warn if multiple are provided; only the first will be effective for matching.
        if btp.spec.target_refs.len() > 1 {
            tracing::warn!(
                policy = %resource_ref.key(),
                count = btp.spec.target_refs.len(),
                "BackendTLSPolicy has {} targetRefs, but Gateway API v1.5 requires exactly one; \
                 only the first targetRef is guaranteed to be conformant",
                btp.spec.target_refs.len()
            );
        }

        // --- Conflict detection: register target refs ---
        let target_manager = get_backend_tls_policy_target_manager();

        // Find old siblings BEFORE clearing (they may need status re-evaluation
        // if this policy's targetRef changed).
        let old_siblings = get_sibling_policies(&resource_ref);

        // Clear old target registrations and re-register with current targetRefs.
        target_manager.clear_resource_refs(&resource_ref);

        let policy_ns_str = btp.metadata.namespace.as_deref().unwrap_or("default");
        for target_ref in &btp.spec.target_refs {
            let target_key =
                make_target_conflict_key(policy_ns_str, &target_ref.name, target_ref.section_name.as_deref());
            target_manager.add_ref(target_key, resource_ref.clone());
        }

        // Store creation timestamp for conflict precedence sorting.
        let policy_resource_key = resource_ref.resource_key();
        let ts_string = btp.metadata.creation_timestamp.as_ref().map(|t| t.0.to_rfc3339());
        set_policy_timestamp(&policy_resource_key, ts_string.as_deref());

        // Requeue old siblings whose conflict status may have changed.
        for sibling in old_siblings {
            ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
        }

        // Clear old references first to handle updates safely.
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        btp.spec.resolved_ca_certificates = None;
        btp.spec.resolved_client_certificate = None;

        btp.spec.use_system_ca = btp.spec.validation.well_known_ca_certificates.is_some();

        let policy_ns = btp.metadata.namespace.as_ref();
        let mut resolved_ca_secrets = Vec::new();

        if let Some(refs) = &btp.spec.validation.ca_certificate_refs {
            for ca_ref in refs {
                if Self::is_secret_ref(ca_ref) {
                    let secret_key = format_secret_key(policy_ns, &ca_ref.name);
                    ctx.secret_ref_manager()
                        .add_ref(secret_key.clone(), resource_ref.clone());

                    if let Some(secret) = get_secret(policy_ns.map(|s| s.as_str()), &ca_ref.name) {
                        resolved_ca_secrets.push(secret);
                    } else {
                        tracing::info!(
                            policy = %resource_ref.key(),
                            secret_key = %secret_key,
                            "CA Secret not found yet, BackendTLSPolicy will be reprocessed when Secret arrives"
                        );
                    }
                } else if Self::is_configmap_ref(ca_ref) {
                    let cm_key = format_secret_key(policy_ns, &ca_ref.name);
                    ctx.secret_ref_manager().add_ref(cm_key.clone(), resource_ref.clone());

                    if let Some(cm) = get_configmap(policy_ns.map(|s| s.as_str()), &ca_ref.name) {
                        if let Some(data) = &cm.data {
                            if let Some(ca_pem) = data.get(CA_CERT) {
                                let mut secret_data = BTreeMap::new();
                                secret_data.insert(CA_CERT.to_string(), ByteString(ca_pem.as_bytes().to_vec()));
                                let synthetic_secret = k8s_openapi::api::core::v1::Secret {
                                    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                                        name: Some(ca_ref.name.clone()),
                                        namespace: policy_ns.cloned(),
                                        ..Default::default()
                                    },
                                    data: Some(secret_data),
                                    ..Default::default()
                                };
                                resolved_ca_secrets.push(synthetic_secret);
                            }
                        }
                    } else {
                        tracing::info!(
                            policy = %resource_ref.key(),
                            configmap_key = %cm_key,
                            "CA ConfigMap not found yet, BackendTLSPolicy will be reprocessed when ConfigMap arrives"
                        );
                    }
                }
            }
        }

        if !resolved_ca_secrets.is_empty() {
            btp.spec.resolved_ca_certificates = Some(resolved_ca_secrets);
        }

        match btp.client_certificate_secret_ref() {
            Ok(Some(client_ref)) => {
                let client_ns = client_ref.namespace.as_ref().or(policy_ns);
                let secret_key = format_secret_key(client_ns, &client_ref.name);
                ctx.secret_ref_manager()
                    .add_ref(secret_key.clone(), resource_ref.clone());

                if let Some(secret) = get_secret(client_ns.map(|s| s.as_str()), &client_ref.name) {
                    btp.spec.resolved_client_certificate = Some(secret);
                } else {
                    tracing::info!(
                        policy = %resource_ref.key(),
                        secret_key = %secret_key,
                        "Client certificate Secret not found yet, BackendTLSPolicy will be reprocessed when Secret arrives"
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    policy = %resource_ref.key(),
                    err = %err,
                    "Invalid BackendTLSPolicy client certificate option"
                );
            }
        }

        ProcessResult::Continue(btp)
    }

    async fn on_delete_policy(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            btp.metadata.namespace.clone(),
            btp.metadata.name.clone().unwrap_or_default(),
        );

        // Find siblings BEFORE clearing (they may become Accepted after this delete).
        let siblings = get_sibling_policies(&resource_ref);

        // Clear target ref registrations and timestamp.
        get_backend_tls_policy_target_manager().clear_resource_refs(&resource_ref);
        remove_policy_timestamp(&resource_ref.resource_key());

        // Clear secret refs.
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        tracing::debug!(
            policy = %resource_ref.key(),
            sibling_count = siblings.len(),
            "Cleared references on BackendTLSPolicy delete"
        );

        // Requeue sibling policies — a previously Conflicted sibling may now win.
        for sibling in siblings {
            ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
        }
    }

    fn update_policy_status(&self, btp: &mut BackendTLSPolicy, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = btp.metadata.generation;

        // --- Classify CA reference errors with precise reasons ---
        let mut kind_errors = Vec::new(); // → reason: InvalidKind
        let mut ref_errors = Vec::new(); // → reason: InvalidCACertificateRef
        let mut client_cert_errors = Vec::new();

        if !btp.spec.use_system_ca {
            if let Some(refs) = &btp.spec.validation.ca_certificate_refs {
                for ca_ref in refs {
                    if Self::is_secret_ref(ca_ref) || Self::is_configmap_ref(ca_ref) {
                        if !Self::is_ca_resolved(btp, &ca_ref.name) {
                            ref_errors.push(format!(
                                "{} '{}' not found or missing '{}' key",
                                ca_ref.kind, ca_ref.name, CA_CERT
                            ));
                        }
                    } else {
                        kind_errors.push(format!(
                            "Unsupported caCertificateRef kind: {}/{}",
                            ca_ref.group, ca_ref.kind
                        ));
                    }
                }
            }
        }

        match btp.client_certificate_secret_ref() {
            Ok(Some(client_ref)) => {
                if btp.spec.resolved_client_certificate.is_none() {
                    client_cert_errors.push(format!("Client certificate Secret '{}' not found", client_ref.name));
                } else if let Some(secret) = &btp.spec.resolved_client_certificate {
                    if !Self::secret_has_client_cert_pair(secret) {
                        client_cert_errors.push(format!(
                            "Client certificate Secret '{}' must contain '{}' and '{}'",
                            client_ref.name, CERT, KEY
                        ));
                    }
                }
            }
            Ok(None) => {}
            Err(err) => client_cert_errors.push(err),
        }

        // --- Check if ALL CA certificate refs are invalid (Gateway API v1.5 spec:
        // "If ALL CACertificateRefs are invalid, the implementation MUST also ensure
        //  the Accepted Condition is set to status: False, with Reason NoValidCACertificate.")
        let all_ca_refs_invalid = if !btp.spec.use_system_ca {
            let total_ca_refs = btp
                .spec
                .validation
                .ca_certificate_refs
                .as_ref()
                .map_or(0, |refs| refs.len());
            total_ca_refs > 0 && (kind_errors.len() + ref_errors.len()) >= total_ca_refs
        } else {
            false
        };

        // --- Conflict detection ---
        let is_conflicted = if validation_errors.is_empty() {
            let policy_ns = btp.metadata.namespace.as_deref().unwrap_or("default");
            let policy_key = format!("{}/{}", policy_ns, btp.metadata.name.as_deref().unwrap_or(""));
            btp.spec.target_refs.iter().any(|target_ref| {
                let target_key =
                    make_target_conflict_key(policy_ns, &target_ref.name, target_ref.section_name.as_deref());
                !is_policy_winner(&policy_key, &target_key)
            })
        } else {
            false
        };

        // --- Resolve ancestor Gateways ---
        let ancestor_gateways = Self::resolve_ancestor_gateways(btp);

        let status = btp
            .status
            .get_or_insert_with(|| BackendTLSPolicyStatus { ancestors: vec![] });

        // Build Gateway ParentReferences
        let gateway_refs: Vec<ParentReference> = ancestor_gateways
            .iter()
            .map(|(ns, name)| ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some(ns.clone()),
                name: name.clone(),
                section_name: None,
                port: None,
            })
            .collect();

        // Rebuild ancestors list
        status.ancestors.clear();

        for gw_ref in &gateway_refs {
            let mut conditions = Vec::new();

            // Accepted condition — priority: Invalid > NoValidCACertificate > Conflicted > Accepted
            if !validation_errors.is_empty() {
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::ACCEPTED,
                        condition_reasons::INVALID,
                        validation_errors.join("; "),
                        generation,
                    ),
                );
            } else if all_ca_refs_invalid {
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::ACCEPTED,
                        condition_reasons::NO_VALID_CA_CERTIFICATE,
                        "All CA certificate references are invalid",
                        generation,
                    ),
                );
            } else if is_conflicted {
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::ACCEPTED,
                        condition_reasons::POLICY_CONFLICTED,
                        "Policy conflicts with an older policy targeting the same backend",
                        generation,
                    ),
                );
            } else {
                update_condition(
                    &mut conditions,
                    condition_true(
                        condition_types::ACCEPTED,
                        condition_reasons::ACCEPTED,
                        "Policy accepted",
                        generation,
                    ),
                );
            }

            // ResolvedRefs condition — use precise reason per Gateway API conformance
            if !kind_errors.is_empty() {
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::RESOLVED_REFS,
                        condition_reasons::INVALID_KIND,
                        kind_errors.join("; "),
                        generation,
                    ),
                );
            } else if !ref_errors.is_empty() || !client_cert_errors.is_empty() {
                let all_errors: Vec<&str> = ref_errors
                    .iter()
                    .chain(client_cert_errors.iter())
                    .map(|s| s.as_str())
                    .collect();
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::RESOLVED_REFS,
                        condition_reasons::INVALID_CA_CERTIFICATE_REF,
                        all_errors.join("; "),
                        generation,
                    ),
                );
            } else {
                update_condition(
                    &mut conditions,
                    condition_true(
                        condition_types::RESOLVED_REFS,
                        condition_reasons::RESOLVED_REFS,
                        "All references resolved",
                        generation,
                    ),
                );
            }

            status.ancestors.push(PolicyAncestorStatus {
                ancestor_ref: gw_ref.clone(),
                controller_name: self.controller_name.clone(),
                conditions,
            });
        }
    }

    async fn on_change_policy(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            btp.metadata.namespace.clone(),
            btp.metadata.name.clone().unwrap_or_default(),
        );

        // Requeue sibling policies so their conflict status gets re-evaluated.
        let siblings = get_sibling_policies(&resource_ref);
        if !siblings.is_empty() {
            tracing::debug!(
                policy = %resource_ref.key(),
                sibling_count = siblings.len(),
                "Requeuing sibling BackendTLSPolicies for conflict re-evaluation"
            );
            for sibling in siblings {
                ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
            }
        }
    }

    /// Resolve the Gateway ancestors for a BackendTLSPolicy.
    ///
    /// Chain: BTP.targetRef(Service) -> ServiceRefManager(service->routes) -> GatewayRouteIndex(route->gateways)
    ///
    /// Returns deduplicated list of (gateway_ns, gateway_name).
    fn resolve_ancestor_gateways(btp: &BackendTLSPolicy) -> Vec<(String, String)> {
        use crate::core::controller::conf_mgr::sync_runtime::resource_processor::gateway_route_index::get_gateway_route_index;
        use crate::core::controller::conf_mgr::sync_runtime::resource_processor::service_ref::get_service_ref_manager;

        let policy_ns = btp.metadata.namespace.as_deref().unwrap_or("default");
        let service_ref_manager = get_service_ref_manager();
        let route_index = get_gateway_route_index();

        let mut gateways = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for target_ref in &btp.spec.target_refs {
            // Per Gateway API spec, targetRef can only reference resources in the same namespace.
            let service_key = format!("{}/{}", policy_ns, target_ref.name);

            // Step 1: Service -> Routes
            let routes = service_ref_manager.get_refs(&service_key);

            for route_ref in &routes {
                let route_key = route_ref.resource_key();

                // Step 2: Route -> Gateways
                let gw_keys = route_index.get_gateways_for_route(route_ref.kind, &route_key);

                for gw_key in gw_keys {
                    if seen.insert(gw_key.clone()) {
                        if let Some((ns, name)) = gw_key.split_once('/') {
                            gateways.push((ns.to_string(), name.to_string()));
                        }
                    }
                }
            }
        }

        gateways
    }

    #[inline]
    fn is_secret_ref(ca_ref: &BackendTLSPolicyCACertificateRef) -> bool {
        is_core_api_group(&ca_ref.group) && ca_ref.kind == "Secret"
    }

    #[inline]
    fn is_configmap_ref(ca_ref: &BackendTLSPolicyCACertificateRef) -> bool {
        is_core_api_group(&ca_ref.group) && ca_ref.kind == "ConfigMap"
    }

    #[inline]
    fn is_ca_resolved(btp: &BackendTLSPolicy, name: &str) -> bool {
        btp.spec
            .resolved_ca_certificates
            .as_ref()
            .is_some_and(|secrets| secrets.iter().any(|s| s.metadata.name.as_deref() == Some(name)))
    }

    #[inline]
    fn secret_has_client_cert_pair(secret: &k8s_openapi::api::core::v1::Secret) -> bool {
        secret
            .data
            .as_ref()
            .is_some_and(|d| d.contains_key(CERT) && d.contains_key(KEY))
    }

    #[allow(dead_code)]
    #[inline]
    fn secret_has_ca_cert(secret: &k8s_openapi::api::core::v1::Secret) -> bool {
        secret.data.as_ref().is_some_and(|d| d.contains_key(CA_CERT))
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<BackendTLSPolicy> for BackendTlsPolicyHandler {
    fn validate(&self, btp: &BackendTLSPolicy, _ctx: &HandlerContext) -> Vec<String> {
        self.validate_policy(btp)
    }

    async fn parse(&self, btp: BackendTLSPolicy, ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        self.parse_policy(btp, ctx).await
    }

    async fn on_change(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        self.on_change_policy(btp, ctx).await;
    }

    async fn on_delete(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        self.on_delete_policy(btp, ctx).await;
    }

    fn update_status(&self, btp: &mut BackendTLSPolicy, ctx: &HandlerContext, validation_errors: &[String]) {
        self.update_policy_status(btp, ctx, validation_errors);
    }
}

/// BackendTLSPolicy v1alpha3 compat handler.
///
/// Keeps the older watch endpoint working while reusing the same validation and
/// runtime preprocessing logic as the v1 handler.
pub struct BackendTlsPolicyAlpha3Handler {
    inner: BackendTlsPolicyHandler,
}

impl BackendTlsPolicyAlpha3Handler {
    pub fn new(controller_name: String) -> Self {
        Self {
            inner: BackendTlsPolicyHandler::new(controller_name),
        }
    }
}

impl Default for BackendTlsPolicyAlpha3Handler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<BackendTLSPolicyAlpha3> for BackendTlsPolicyAlpha3Handler {
    fn validate(&self, btp: &BackendTLSPolicyAlpha3, _ctx: &HandlerContext) -> Vec<String> {
        self.inner.validate_policy(&btp.clone().into())
    }

    async fn parse(&self, btp: BackendTLSPolicyAlpha3, ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicyAlpha3> {
        match self.inner.parse_policy(btp.into(), ctx).await {
            ProcessResult::Continue(v1) => ProcessResult::Continue(v1.into()),
            ProcessResult::Skip { reason } => ProcessResult::Skip { reason },
        }
    }

    async fn on_change(&self, btp: &BackendTLSPolicyAlpha3, ctx: &HandlerContext) {
        let v1: BackendTLSPolicy = btp.clone().into();
        self.inner.on_change_policy(&v1, ctx).await;
    }

    async fn on_delete(&self, btp: &BackendTLSPolicyAlpha3, ctx: &HandlerContext) {
        let v1: BackendTLSPolicy = btp.clone().into();
        self.inner.on_delete_policy(&v1, ctx).await;
    }

    fn update_status(&self, btp: &mut BackendTLSPolicyAlpha3, ctx: &HandlerContext, validation_errors: &[String]) {
        let mut v1: BackendTLSPolicy = btp.clone().into();
        self.inner.update_policy_status(&mut v1, ctx, validation_errors);
        *btp = v1.into();
    }
}
