//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources with Gateway API standard status management.

use std::collections::BTreeMap;

use k8s_openapi::ByteString;

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::backend_tls_policy_ref::make_target_conflict_key;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key, update_condition,
    HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use crate::types::prelude_resources::{BackendTLSPolicy, BackendTLSPolicyAlpha3};
use crate::types::resources::backend_tls_policy::{
    BackendTLSPolicyCACertificateRef, BackendTLSPolicyStatus, PolicyAncestorStatus,
};
use crate::types::resources::common::{is_core_api_group, ParentReference};
use crate::types::ResourceKind;

/// Decide whether sibling BackendTLSPolicies (those targeting the same
/// resource) must be requeued after this policy was processed.
///
/// Returns `true` when either of the precedence-relevant inputs changed:
///
/// - **Target set changed** (`old_target_keys != new_target_keys`): old
///   siblings may no longer conflict with this policy and new siblings need
///   to (re)evaluate the conflict against this policy.
/// - **Creation timestamp changed** with target set unchanged: this happens
///   when a BackendTLSPolicy is delete+recreated (same name, possibly with
///   updated `validation` block) within one PollWatcher tick. The new
///   timestamp shifts precedence ordering, so previously-`Conflicted` siblings
///   may now win — and previously-`Accepted` siblings may now lose.
///
/// Returns `false` for no-op updates (e.g. a `resourceVersion` bump with
/// identical targets and identical timestamp). Skipping requeue here is
/// important: it prevents A→B→A→B ping-pong cycles between conflicting
/// policies that would otherwise reprocess each other forever.
fn should_requeue_siblings(
    old_target_keys: &[String],
    new_target_keys: &[String],
    old_timestamp: Option<&str>,
    new_timestamp: Option<&str>,
) -> bool {
    old_target_keys != new_target_keys || old_timestamp != new_timestamp
}

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
        // The snapshot → clear → re-register sequence below is not atomic at
        // the store level, but is safe because the BackendTLSPolicy
        // FileSystemResourceController processes one policy key at a time on
        // its receive loop — no concurrent parse_policy runs against the same
        // policy_resource_key. If the framework is ever changed to parallelise
        // per-key work this would need a finer lock.
        let target_manager = ctx.runtime().backend_tls_policy.target_mgr();

        // Snapshot old target keys, siblings, and creation timestamp BEFORE
        // mutating any state, so we can detect whether either the target set
        // or the precedence-relevant metadata changed.
        let old_target_keys = {
            let mut keys = target_manager.get_dependencies(&resource_ref.key());
            keys.sort();
            keys
        };
        let old_siblings = ctx.runtime().backend_tls_policy.get_siblings(&resource_ref);
        let policy_resource_key = resource_ref.resource_key();
        let old_timestamp = ctx.runtime().backend_tls_policy.timestamp_of(&policy_resource_key);

        // Clear old target registrations and re-register with current targetRefs.
        target_manager.clear_resource_refs(&resource_ref);

        let policy_ns_str = btp.metadata.namespace.as_deref().unwrap_or("default");
        let mut new_target_keys = Vec::with_capacity(btp.spec.target_refs.len());
        for target_ref in &btp.spec.target_refs {
            let target_key =
                make_target_conflict_key(policy_ns_str, &target_ref.name, target_ref.section_name.as_deref());
            target_manager.add_ref(target_key.clone(), resource_ref.clone());
            new_target_keys.push(target_key);
        }
        new_target_keys.sort();

        // Store creation timestamp for conflict precedence sorting.
        let new_timestamp = btp.metadata.creation_timestamp.as_ref().map(|t| t.0.to_rfc3339());
        ctx.runtime()
            .backend_tls_policy
            .set_timestamp(&policy_resource_key, new_timestamp.as_deref());

        // Decide whether to requeue siblings. Two precedence-relevant signals:
        //   (a) target set changed — old siblings may no longer conflict, new
        //       siblings need to (re)evaluate against this policy.
        //   (b) target set is unchanged but creation_timestamp changed — this
        //       happens when a BTP is delete+recreated (same name) within one
        //       PollWatcher tick. The new timestamp shifts precedence ordering,
        //       so previously-Conflicted siblings may now win (or lose).
        //
        // Otherwise (no-op update — e.g. resourceVersion bump with identical
        // targets and timestamp) skip requeue to avoid the A→B→A→B ping-pong
        // cycle between conflicting policies.
        if should_requeue_siblings(
            &old_target_keys,
            &new_target_keys,
            old_timestamp.as_deref(),
            new_timestamp.as_deref(),
        ) {
            for sibling in &old_siblings {
                ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
            }
            // Also requeue NEW siblings — they may now conflict with this policy.
            let new_siblings = ctx.runtime().backend_tls_policy.get_siblings(&resource_ref);
            for sibling in &new_siblings {
                if !old_siblings.contains(sibling) {
                    ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
                }
            }
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

                    if let Some(secret) = ctx
                        .runtime()
                        .secret_store
                        .get(policy_ns.map(|s| s.as_str()), &ca_ref.name)
                    {
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

                    if let Some(cm) = ctx
                        .runtime()
                        .configmap_store
                        .get(policy_ns.map(|s| s.as_str()), &ca_ref.name)
                    {
                        let ca_pem = cm
                            .data
                            .as_ref()
                            .and_then(|d| d.get(CA_CERT))
                            .filter(|v| !v.trim().is_empty());

                        if let Some(ca_pem) = ca_pem {
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

                if let Some(secret) = ctx
                    .runtime()
                    .secret_store
                    .get(client_ns.map(|s| s.as_str()), &client_ref.name)
                {
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
        let siblings = ctx.runtime().backend_tls_policy.get_siblings(&resource_ref);

        // Clear target ref registrations and timestamp.
        ctx.runtime()
            .backend_tls_policy
            .target_mgr()
            .clear_resource_refs(&resource_ref);
        ctx.runtime()
            .backend_tls_policy
            .remove_timestamp(&resource_ref.resource_key());

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
                !_ctx.runtime().backend_tls_policy.is_winner(&policy_key, &target_key)
            })
        } else {
            false
        };

        // --- Resolve ancestor Gateways ---
        let ancestor_gateways = Self::resolve_ancestor_gateways(btp, _ctx);

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

    async fn on_change_policy(&self, _btp: &BackendTLSPolicy, _ctx: &HandlerContext) {
        // Sibling requeue is handled in parse_policy() (only when targetRefs
        // actually change), so nothing to do here. Unconditionally requeuing
        // siblings in both parse and on_change was causing A→B→A→B ping-pong
        // cycles between conflicting policies.
    }

    /// Resolve the Gateway ancestors for a BackendTLSPolicy.
    ///
    /// Chain: BTP.targetRef(Service) -> ServiceRefManager(service->routes) -> GatewayRouteIndex(route->gateways)
    ///
    /// Returns deduplicated, sorted list of (gateway_ns, gateway_name), capped at 16
    /// per Gateway API `+kubebuilder:validation:MaxItems=16` on `status.ancestors`.
    fn resolve_ancestor_gateways(btp: &BackendTLSPolicy, ctx: &HandlerContext) -> Vec<(String, String)> {
        // Gateway API PolicyAncestorStatus maxItems=16.
        const MAX_POLICY_ANCESTORS: usize = 16;

        let policy_ns = btp.metadata.namespace.as_deref().unwrap_or("default");
        let service_ref_manager = &ctx.runtime().service_ref_mgr;
        let route_index = &ctx.runtime().gateway_route_index;

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
                    seen.insert(gw_key);
                }
            }
        }

        // Sort for stable truncation: same 16 Gateways selected on every reconcile.
        let mut keys: Vec<String> = seen.into_iter().collect();
        keys.sort_unstable();

        if keys.len() > MAX_POLICY_ANCESTORS {
            tracing::warn!(
                policy = %format!("{}/{}", policy_ns, btp.metadata.name.as_deref().unwrap_or("")),
                total = keys.len(),
                "BackendTLSPolicy ancestors truncated to {} per Gateway API limit",
                MAX_POLICY_ANCESTORS,
            );
            keys.truncate(MAX_POLICY_ANCESTORS);
        }

        keys.into_iter()
            .filter_map(|k| k.split_once('/').map(|(ns, name)| (ns.to_string(), name.to_string())))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::SecretRefManager;
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::resources::backend_tls_policy::{
        BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation, WellKnownCACertificates,
    };
    use crate::types::ResourceKind;
    use chrono::{DateTime, Utc};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use kube::api::ObjectMeta;
    use std::sync::Arc;

    // -------- should_requeue_siblings (pure decision) --------

    #[test]
    fn should_requeue_when_target_set_changes() {
        assert!(should_requeue_siblings(
            &["ns/svc-a".to_string()],
            &["ns/svc-b".to_string()],
            Some("2024-01-01T00:00:00+00:00"),
            Some("2024-01-01T00:00:00+00:00"),
        ));
    }

    #[test]
    fn should_requeue_when_timestamp_changes_with_same_targets() {
        // The bug: BTP delete+recreated within one PollWatcher tick — same
        // name, same targets, but a new creation_timestamp must trigger
        // sibling re-evaluation.
        let targets = ["ns/my-svc".to_string()];
        assert!(should_requeue_siblings(
            &targets,
            &targets,
            Some("2024-01-01T00:00:00+00:00"),
            Some("2024-06-01T00:00:00+00:00"),
        ));
    }

    #[test]
    fn should_not_requeue_on_noop_update() {
        // Steady state: identical targets, identical timestamp (e.g. a
        // resourceVersion bump from a status write). Skipping requeue is
        // what prevents the A→B→A→B ping-pong between conflicting policies.
        let targets = ["ns/my-svc".to_string()];
        assert!(!should_requeue_siblings(
            &targets,
            &targets,
            Some("2024-01-01T00:00:00+00:00"),
            Some("2024-01-01T00:00:00+00:00"),
        ));
    }

    #[test]
    fn should_requeue_when_old_timestamp_was_missing() {
        // First registration after a controller restart: old timestamp is
        // None, new timestamp is Some. Treat as a precedence-relevant change.
        let targets = ["ns/my-svc".to_string()];
        assert!(should_requeue_siblings(
            &targets,
            &targets,
            None,
            Some("2024-01-01T00:00:00+00:00"),
        ));
    }

    // -------- parse_policy delete+recreate path (state-level) --------

    fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(
            secret_ref_manager,
            None,
            None,
            TriggerChain::default(),
            5,
            20,
            runtime.clone(),
        );
        (ctx, runtime)
    }

    fn make_btp(ns: &str, name: &str, target_svc: &str, creation_ts: &str) -> BackendTLSPolicy {
        let ts: DateTime<Utc> = creation_ts.parse().expect("valid RFC3339 timestamp in test");
        BackendTLSPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                creation_timestamp: Some(Time(ts)),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: target_svc.to_string(),
                    section_name: None,
                }],
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "backend.example.com".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: Some(WellKnownCACertificates::System),
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: false,
            },
            status: None,
        }
    }

    /// Regression test for the precedence-staleness bug.
    ///
    /// Setup: two BTPs (winner, sibling) both target the same Service.
    /// `winner` has the older creation_timestamp, so it wins; `sibling` is
    /// `Conflicted`.
    ///
    /// Then `winner` is delete+recreated within one PollWatcher tick — same
    /// name, same targets, but a NEWER creation_timestamp than `sibling`.
    /// After processing, `sibling` should now win, and `parse_policy` must
    /// have observed the precedence flip via the `should_requeue_siblings`
    /// helper so siblings get requeued.
    ///
    /// Without the fix, the timestamp-only change is ignored and `sibling`
    /// keeps a stale `Conflicted` status until the next unrelated event.
    ///
    /// Scope: this asserts on the **state-level outcome** (precedence in the
    /// `BackendTLSPolicyState` flips correctly).  It does not directly observe
    /// `ctx.requeue` invocation, because the test runtime has no processor
    /// registered for `BackendTLSPolicy` (so requeue is a no-op log).  The
    /// `should_requeue_siblings` helper is independently covered by the four
    /// pure-decision tests above.
    #[tokio::test]
    async fn parse_policy_requeues_siblings_when_winner_replaced_with_newer_timestamp() {
        let (ctx, runtime) = make_ctx();
        let handler = BackendTlsPolicyHandler::default();

        // 1. Initial state: winner is OLDER, sibling is YOUNGER.
        let winner = make_btp("ns", "winner", "my-svc", "2024-01-01T00:00:00+00:00");
        let sibling = make_btp("ns", "sibling", "my-svc", "2024-06-01T00:00:00+00:00");

        let _ = handler.parse_policy(winner, &ctx).await;
        let _ = handler.parse_policy(sibling, &ctx).await;

        let target_key = make_target_conflict_key("ns", "my-svc", None);
        assert!(
            runtime.backend_tls_policy.is_winner("ns/winner", &target_key),
            "older winner should be the precedence winner before the recreate"
        );
        assert!(
            !runtime.backend_tls_policy.is_winner("ns/sibling", &target_key),
            "younger sibling should be conflicted before the recreate"
        );

        // 2. Sanity: a no-op reparse of the winner does NOT change precedence
        //    state and should be detected as not-requeue-worthy by the helper.
        //    (Verified by re-checking is_winner — same result.)
        let winner_noop = make_btp("ns", "winner", "my-svc", "2024-01-01T00:00:00+00:00");
        let _ = handler.parse_policy(winner_noop, &ctx).await;
        assert!(
            runtime.backend_tls_policy.is_winner("ns/winner", &target_key),
            "noop reparse must not flip precedence"
        );

        // 3. Bug scenario: winner is deleted and recreated within one tick
        //    with the SAME name and SAME targets but a NEWER timestamp than
        //    `sibling`. After parse, the timestamp store reflects the new
        //    timestamp, and `sibling` should now win on creation_timestamp
        //    ordering.
        let winner_recreated = make_btp("ns", "winner", "my-svc", "2024-12-01T00:00:00+00:00");
        let old_ts_before_recreate = runtime.backend_tls_policy.timestamp_of("ns/winner");
        let _ = handler.parse_policy(winner_recreated, &ctx).await;
        let new_ts_after_recreate = runtime.backend_tls_policy.timestamp_of("ns/winner");

        assert_ne!(
            old_ts_before_recreate, new_ts_after_recreate,
            "timestamp store must reflect the recreated BTP's new creation_timestamp"
        );

        // The recreated winner is now YOUNGER than sibling, so sibling must
        // now win the target on precedence — confirming the precedence flip
        // that should_requeue_siblings detects via the timestamp delta.
        assert!(
            runtime.backend_tls_policy.is_winner("ns/sibling", &target_key),
            "sibling should now win after the winner was recreated with a newer timestamp"
        );
        assert!(
            !runtime.backend_tls_policy.is_winner("ns/winner", &target_key),
            "recreated winner is now the younger policy and should be conflicted"
        );

        // 4. Direct check on the decision helper: with same target set but a
        //    different timestamp, siblings MUST be requeued. This is the
        //    specific code path that was missing before the fix.
        let targets = vec![target_key.clone()];
        assert!(should_requeue_siblings(
            &targets,
            &targets,
            old_ts_before_recreate.as_deref(),
            new_ts_after_recreate.as_deref(),
        ));
    }

    #[test]
    fn get_siblings_returns_other_policies_targeting_same_service() {
        let runtime = ControllerRuntime::default();
        let target_key = make_target_conflict_key("ns", "my-svc", None);

        let winner = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            Some("ns".to_string()),
            "winner".to_string(),
        );
        let sibling = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            Some("ns".to_string()),
            "sibling".to_string(),
        );

        runtime
            .backend_tls_policy
            .target_mgr()
            .add_ref(target_key.clone(), winner.clone());
        runtime
            .backend_tls_policy
            .target_mgr()
            .add_ref(target_key, sibling.clone());

        let siblings_of_winner = runtime.backend_tls_policy.get_siblings(&winner);
        assert_eq!(siblings_of_winner, vec![sibling]);
    }
}
