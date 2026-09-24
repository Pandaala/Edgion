//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources with Gateway API standard status management.

use std::collections::{BTreeMap, BTreeSet};

use k8s_openapi::api::core::v1::Service;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;

use crate::conf_mgr::sync_runtime::resource_processor::backend_tls_policy_ref::make_target_conflict_key;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::backend_capability::classify_raw_backend;
use crate::conf_mgr::sync_runtime::resource_processor::status_pipeline::filter_current_native_attachment_status;
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key, update_condition,
    HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use chrono::{DateTime, Utc};
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::prelude_resources::BackendTLSPolicy;
#[cfg(test)]
use edgion_resources::prelude_resources::EdgionBackend;
use edgion_resources::resources::backend_tls_policy::{
    parse_client_certificate_option, BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyStatus,
    BackendTLSPolicyTargetRef,
};
use edgion_resources::resources::common::{is_core_api_group, ParentReference, PolicyAncestorStatus};
use edgion_resources::resources::edgion_backend_traffic_policy::{
    PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass,
};
use edgion_resources::ResourceKind;

#[cfg(test)]
fn service_target_key(namespace: &str, name: &str) -> PolicyTargetKey {
    PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: namespace.to_string(),
        name: name.to_string(),
    }
}

fn btp_target_base_key(key: &str) -> &str {
    key.split("::").next().unwrap_or(key)
}

#[derive(Default)]
struct PolicyTargetEvaluation {
    target: Option<PolicyTargetKey>,
    target_class: Option<ResolvedPolicyTargetClass>,
    missing_targets: BTreeSet<PolicyTargetKey>,
    errors: Vec<String>,
}

fn raw_service_exists(target: &PolicyTargetKey, ctx: &HandlerContext) -> bool {
    let Some(processor) = ctx.runtime().processor_registry.get("Service") else {
        return true;
    };
    processor
        .get_raw_any(&format!("{}/{}", target.namespace, target.name))
        .and_then(|raw| raw.downcast::<Service>().ok())
        .is_some()
}

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
    old_timestamp: Option<DateTime<Utc>>,
    new_timestamp: Option<DateTime<Utc>>,
) -> bool {
    old_target_keys != new_target_keys || old_timestamp != new_timestamp
}

/// Requeue every `EdgionBackendTrafficPolicy` that depends on one of this BackendTLSPolicy's
/// target Services, so a BackendTLSPolicy add/edit/delete re-evaluates the EBTP
/// `upstreamAuthority` cross-object admission rule (`edgion_backend_traffic_policy.rs::validate`)
/// instead of leaving a stale `Accepted`/`Conflicted` status. Without this edge, EBTP is only
/// requeued by its own object change, an EBTP sibling, or a Service change — never by the
/// BackendTLSPolicy it cross-references — so the rule is order-dependent (only catches an
/// out-of-pattern hostname if the EBTP happens to reconcile again for an unrelated reason) and
/// fails open on the canonical "apply the offending BackendTLSPolicy after the EBTP already
/// exists" ops sequence.
///
/// `EdgionBackendTrafficPolicyHandler::parse` registers each EBTP into
/// `ctx.runtime().edgion_backend_traffic_policy.target_mgr()` under
/// its typed `PolicyTargetKey` identity (`"/Service/ns/svc"` for a core Service). This function
/// looks up that same key per targetRef; section names on the BackendTLSPolicy side are irrelevant
/// to the EBTP index.
///
/// Note: an EBTP only enters that index when its own spec is locally valid (see `spec_valid` in
/// `edgion_backend_traffic_policy.rs::parse`) — a spec-invalid EBTP is already `Accepted=False`
/// and intentionally does not participate.
async fn requeue_dependent_ebtps(meta: &ObjectMeta, target_refs: &[BackendTLSPolicyTargetRef], ctx: &HandlerContext) {
    let ns = meta.namespace.as_deref().unwrap_or("default");
    for target_ref in target_refs {
        let Ok(target_key) = target_ref.canonical_key(ns) else {
            continue;
        };
        for ebtp_ref in ctx
            .runtime()
            .edgion_backend_traffic_policy
            .target_mgr()
            .get_refs(&target_key.to_string())
        {
            ctx.requeue(ebtp_ref.kind_str(), ebtp_ref.resource_key()).await;
        }
    }
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
    fn validate_policy(&self, spec: &BackendTLSPolicySpec) -> Vec<String> {
        let mut errors = Vec::new();

        if spec.target_refs.len() != 1 {
            errors.push("targetRefs must contain exactly one targetRef".to_string());
        }

        let validation = &spec.validation;
        let has_ca_refs = validation
            .ca_certificate_refs
            .as_ref()
            .is_some_and(|refs| !refs.is_empty());
        let has_well_known = validation.well_known_ca_certificates.is_some();
        if matches!(
            validation.well_known_ca_certificates.as_ref(),
            Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::Other(_))
        ) {
            errors.push("validation.wellKnownCACertificates is unsupported; only System can be used".to_string());
        }

        if has_ca_refs && has_well_known {
            errors.push("caCertificateRefs and wellKnownCACertificates are mutually exclusive".to_string());
        }

        if !has_ca_refs && !has_well_known {
            errors.push("must specify either caCertificateRefs or wellKnownCACertificates".to_string());
        }

        if let Err(error) = parse_client_certificate_option(spec.options.as_ref()) {
            errors.push(error);
        }

        if let Some(refs) = &validation.ca_certificate_refs {
            let mut seen_refs = BTreeSet::new();
            for ca_ref in refs {
                if ca_ref.namespace.is_some() {
                    errors.push(format!(
                        "caCertificateRef {}/{} must not specify namespace; BackendTLSPolicy references are same-namespace",
                        ca_ref.kind, ca_ref.name
                    ));
                }
                let canonical_group = if is_core_api_group(&ca_ref.group) {
                    ""
                } else {
                    ca_ref.group.as_str()
                };
                if !seen_refs.insert((canonical_group, ca_ref.kind.as_str(), ca_ref.name.as_str())) {
                    errors.push(format!(
                        "duplicate caCertificateRef {}/{} is not allowed",
                        ca_ref.kind, ca_ref.name
                    ));
                }
            }
        }

        errors
    }

    fn evaluate_target(
        &self,
        meta: &ObjectMeta,
        spec: &BackendTLSPolicySpec,
        ctx: &HandlerContext,
    ) -> PolicyTargetEvaluation {
        let mut evaluation = PolicyTargetEvaluation {
            errors: self.validate_policy(spec),
            ..PolicyTargetEvaluation::default()
        };
        if spec.target_refs.len() != 1 {
            return evaluation;
        }

        let namespace = meta.namespace.as_deref().unwrap_or("default");
        let target_ref = &spec.target_refs[0];
        let target = match target_ref.canonical_key(namespace) {
            Ok(target) => target,
            Err(error) => {
                evaluation.errors.push(error);
                return evaluation;
            }
        };
        if target.target_kind == PolicyTargetKind::EdgionBackend && target_ref.section_name.is_some() {
            evaluation
                .errors
                .push("sectionName is not supported for EdgionBackend targets".to_string());
        }

        match target.target_kind {
            PolicyTargetKind::Service => {
                if raw_service_exists(&target, ctx) {
                    evaluation.target_class = Some(ResolvedPolicyTargetClass::Service);
                } else {
                    evaluation.missing_targets.insert(target.clone());
                }
            }
            PolicyTargetKind::EdgionBackend => match classify_raw_backend(&target, ctx) {
                Ok(Some(capability)) if capability.supports_tls_policy => {
                    evaluation.target_class = Some(capability.target_class)
                }
                Ok(Some(_)) => evaluation.errors.push(format!(
                    "EdgionBackend {}/{} must use an HTTPS endpoint for BackendTLSPolicy",
                    target.namespace, target.name
                )),
                Ok(None) => {
                    evaluation.missing_targets.insert(target.clone());
                }
                Err(error) => evaluation.errors.push(error),
            },
        }
        evaluation.target = Some(target);
        if !evaluation.errors.is_empty() || !evaluation.missing_targets.is_empty() {
            evaluation.target_class = None;
        }
        evaluation
    }

    fn all_runtime_refs_resolved(spec: &BackendTLSPolicySpec) -> bool {
        let ca_resolved = spec.use_system_ca
            || spec.validation.ca_certificate_refs.as_ref().is_some_and(|refs| {
                !refs.is_empty()
                    && refs.iter().all(|ca_ref| {
                        (Self::is_secret_ref(ca_ref) || Self::is_configmap_ref(ca_ref))
                            && ca_ref.namespace.is_none()
                            && Self::is_ca_resolved(spec, &ca_ref.name)
                    })
            });
        let client_resolved = match parse_client_certificate_option(spec.options.as_ref()) {
            Ok(Some(_)) => spec
                .resolved_client_certificate
                .as_ref()
                .is_some_and(Self::secret_has_client_cert_pair),
            Ok(None) => true,
            Err(_) => false,
        };
        ca_resolved && client_resolved
    }

    async fn parse_policy(&self, mut btp: BackendTLSPolicy, ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            btp.metadata.namespace.clone(),
            btp.metadata.name.clone().unwrap_or_default(),
        );

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

        // Clear every controller-owned staging field and dependency edge before
        // rebuilding the current generation. A delete->create coalesced by the
        // workqueue must not retain a previous target, marker, or Secret.
        target_manager.clear_resource_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        btp.spec.resolved_target_class = None;
        btp.spec.resolved_ca_certificates = None;
        btp.spec.resolved_client_certificate = None;

        // Register a syntactically canonical target dependency before checking
        // existence. Creation of a currently-missing target must wake this policy.
        let policy_ns_str = btp.metadata.namespace.as_deref().unwrap_or("default");
        for target in btp
            .spec
            .target_refs
            .iter()
            .filter_map(|target_ref| target_ref.canonical_key(policy_ns_str).ok())
        {
            match target.target_kind {
                PolicyTargetKind::Service => ctx
                    .runtime()
                    .service_ref_mgr
                    .add_ref(format!("{}/{}", target.namespace, target.name), resource_ref.clone()),
                PolicyTargetKind::EdgionBackend => ctx
                    .runtime()
                    .backend_ref_mgr
                    .add_ref(target.to_string(), resource_ref.clone()),
            }
        }

        btp.spec.use_system_ca = matches!(
            btp.spec.validation.well_known_ca_certificates.as_ref(),
            Some(edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System)
        );

        let policy_ns = btp.metadata.namespace.as_ref();
        let mut resolved_ca_secrets = Vec::new();

        if let Some(refs) = &btp.spec.validation.ca_certificate_refs {
            for ca_ref in refs {
                if ca_ref.namespace.is_some() {
                    continue;
                }
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
                    let cm_key = super::super::format_configmap_ref_key(policy_ns, &ca_ref.name);
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

        let evaluation = self.evaluate_target(&btp.metadata, &btp.spec, ctx);
        let mut new_target_keys = Vec::new();
        if evaluation.target_class.is_some() && Self::all_runtime_refs_resolved(&btp.spec) {
            if let Some(target) = &evaluation.target {
                let section = btp.spec.target_refs[0].section_name.as_deref();
                let conflict_key = make_target_conflict_key(target, section);
                target_manager.add_ref(conflict_key.clone(), resource_ref.clone());
                new_target_keys.push(conflict_key);
                btp.spec.resolved_target_class = evaluation.target_class;
            }
        }
        new_target_keys.sort();

        let new_timestamp = btp.metadata.creation_timestamp.as_ref().map(|t| t.0);
        ctx.runtime()
            .backend_tls_policy
            .set_timestamp(&policy_resource_key, new_timestamp);

        if should_requeue_siblings(&old_target_keys, &new_target_keys, old_timestamp, new_timestamp) {
            for sibling in &old_siblings {
                ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
            }
            for sibling in ctx.runtime().backend_tls_policy.get_siblings(&resource_ref) {
                if !old_siblings.contains(&sibling) {
                    ctx.requeue(sibling.kind_str(), sibling.resource_key()).await;
                }
            }
        }

        // Notify EBTPs for targets dropped by retargeting or by loss of
        // eligibility. Conflict keys are typed target identities plus an
        // optional section suffix, so their base is directly reusable by the
        // EBTP target index.
        let new_base: std::collections::HashSet<&str> =
            new_target_keys.iter().map(|key| btp_target_base_key(key)).collect();
        let ebtp_target_mgr = ctx.runtime().edgion_backend_traffic_policy.target_mgr();
        for dropped_target in old_target_keys
            .iter()
            .map(|key| btp_target_base_key(key))
            .filter(|key| !new_base.contains(key))
            .collect::<std::collections::HashSet<&str>>()
        {
            for ebtp_ref in ebtp_target_mgr.get_refs(dropped_target) {
                ctx.requeue(ebtp_ref.kind_str(), ebtp_ref.resource_key()).await;
            }
        }

        ProcessResult::Continue(btp)
    }

    async fn on_delete_policy(
        &self,
        meta: &ObjectMeta,
        target_refs: &[BackendTLSPolicyTargetRef],
        ctx: &HandlerContext,
    ) {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            meta.namespace.clone(),
            meta.name.clone().unwrap_or_default(),
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

        // Clear target and Secret dependency edges.
        ctx.runtime().service_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_resource_refs(&resource_ref);
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

        // Requeue dependent EdgionBackendTrafficPolicies — deleting the offending
        // BackendTLSPolicy must clear a stale Accepted=False on the EBTP, otherwise the
        // rejection outlives the object that caused it.
        requeue_dependent_ebtps(meta, target_refs, ctx).await;
    }

    fn update_policy_status(
        &self,
        meta: &ObjectMeta,
        spec: &BackendTLSPolicySpec,
        status: &mut Option<BackendTLSPolicyStatus>,
        ctx: &HandlerContext,
        validation_errors: &[String],
    ) {
        let generation = meta.generation;
        let evaluation = self.evaluate_target(meta, spec, ctx);
        let mut target_errors = validation_errors.to_vec();
        target_errors.extend(evaluation.errors.iter().cloned());
        target_errors.sort();
        target_errors.dedup();

        // --- Classify CA reference errors with precise reasons ---
        let mut kind_errors = Vec::new(); // → reason: InvalidKind
        let mut ref_errors = Vec::new(); // → reason: InvalidCACertificateRef
        let mut client_cert_errors = Vec::new();

        if !spec.use_system_ca {
            if let Some(refs) = &spec.validation.ca_certificate_refs {
                for ca_ref in refs {
                    if Self::is_secret_ref(ca_ref) || Self::is_configmap_ref(ca_ref) {
                        if !Self::is_ca_resolved(spec, &ca_ref.name) {
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

        match parse_client_certificate_option(spec.options.as_ref()) {
            Ok(Some(client_ref)) => match spec.resolved_client_certificate.as_ref() {
                None => client_cert_errors.push(format!("Client certificate Secret '{}' not found", client_ref.name)),
                Some(secret) => {
                    if !Self::secret_has_client_cert_pair(secret) {
                        client_cert_errors.push(format!(
                            "Client certificate Secret '{}' must contain '{}' and '{}'",
                            client_ref.name, CERT, KEY
                        ));
                    }
                }
            },
            Ok(None) => {}
            Err(err) => client_cert_errors.push(err),
        }

        // --- Check if ALL CA certificate refs are invalid (Gateway API v1.5 spec:
        // "If ALL CACertificateRefs are invalid, the implementation MUST also ensure
        //  the Accepted Condition is set to status: False, with Reason NoValidCACertificate.")
        let all_ca_refs_invalid = if !spec.use_system_ca {
            let total_ca_refs = spec
                .validation
                .ca_certificate_refs
                .as_ref()
                .map_or(0, |refs| refs.len());
            total_ca_refs > 0 && (kind_errors.len() + ref_errors.len()) >= total_ca_refs
        } else {
            false
        };

        // --- Conflict detection ---
        let is_conflicted = if target_errors.is_empty() && Self::all_runtime_refs_resolved(spec) {
            let policy_ns = meta.namespace.as_deref().unwrap_or("default");
            let policy_key = format!("{}/{}", policy_ns, meta.name.as_deref().unwrap_or(""));
            evaluation.target.as_ref().is_some_and(|target| {
                let section = spec.target_refs[0].section_name.as_deref();
                let target_key = make_target_conflict_key(target, section);
                !ctx.runtime().backend_tls_policy.is_winner(&policy_key, &target_key)
            })
        } else {
            false
        };

        // --- Resolve ancestor Gateways ---
        let ancestor_gateways = Self::resolve_ancestor_gateways(meta, &evaluation, ctx);

        let status = status.get_or_insert_with(|| BackendTLSPolicyStatus { ancestors: vec![] });

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

        // Rebuild ancestors list, carrying each surviving ancestor's prior
        // conditions forward so update_condition can keep lastTransitionTime
        // stable when the status does not actually flip.
        let old_ancestors = std::mem::take(&mut status.ancestors);
        status.ancestors.extend(
            old_ancestors
                .iter()
                .filter(|ancestor| ancestor.controller_name != self.controller_name)
                .cloned(),
        );

        for gw_ref in &gateway_refs {
            // Match on controller_name + kind as well: in K8s mode the watched
            // status.ancestors may hold entries from other controllers (or
            // GAMMA Service ancestors sharing ns/name) whose conditions must
            // not be adopted.
            let mut conditions = old_ancestors
                .iter()
                .find(|a| {
                    a.controller_name == self.controller_name
                        && a.ancestor_ref.kind == gw_ref.kind
                        && a.ancestor_ref.namespace == gw_ref.namespace
                        && a.ancestor_ref.name == gw_ref.name
                })
                .map(|a| a.conditions.clone())
                .unwrap_or_default();

            // Accepted condition — priority: Invalid > NoValidCACertificate > Conflicted > Accepted
            if !evaluation.missing_targets.is_empty() {
                update_condition(
                    &mut conditions,
                    condition_true(
                        condition_types::ACCEPTED,
                        condition_reasons::ACCEPTED,
                        "Policy accepted pending target resolution",
                        generation,
                    ),
                );
            } else if !target_errors.is_empty() {
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::ACCEPTED,
                        condition_reasons::INVALID,
                        target_errors.join("; "),
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
            if !evaluation.missing_targets.is_empty() {
                update_condition(
                    &mut conditions,
                    condition_false(
                        condition_types::RESOLVED_REFS,
                        "TargetNotFound",
                        "Policy target was not found",
                        generation,
                    ),
                );
            } else if !kind_errors.is_empty() {
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

    async fn on_change_policy(
        &self,
        meta: &ObjectMeta,
        target_refs: &[BackendTLSPolicyTargetRef],
        ctx: &HandlerContext,
    ) {
        // Sibling requeue is handled in parse_policy() (only when targetRefs
        // actually change), so nothing to do here for siblings. Unconditionally
        // requeuing siblings in both parse and on_change was causing A→B→A→B
        // ping-pong cycles between conflicting policies.
        //
        // Dependent EdgionBackendTrafficPolicies are different: `on_change` fires
        // unconditionally on every successful create/update (see
        // `ProcessorHandler::on_change` doc), which is exactly the trigger needed to
        // re-evaluate the `upstreamAuthority` cross-object rule whenever this
        // BackendTLSPolicy's targetRefs or `validation.hostname` may have changed —
        // there is no separate diff/dedup gate to hook into, and requeuing on every
        // change cannot ping-pong because EBTP's own `parse`/`on_change` never write
        // back into `backend_tls_policy`'s state.
        requeue_dependent_ebtps(meta, target_refs, ctx).await;
    }

    /// Resolve the Gateway ancestors for a BackendTLSPolicy.
    ///
    /// Chain: BTP.targetRef(Service) -> ServiceRefManager(service->routes) -> GatewayRouteIndex(route->gateways)
    ///
    /// Returns deduplicated, sorted list of (gateway_ns, gateway_name), capped at 16
    /// per Gateway API `+kubebuilder:validation:MaxItems=16` on `status.ancestors`.
    fn resolve_ancestor_gateways(
        meta: &ObjectMeta,
        evaluation: &PolicyTargetEvaluation,
        ctx: &HandlerContext,
    ) -> Vec<(String, String)> {
        let policy_ns = meta.namespace.as_deref().unwrap_or("default");
        let policy_key = format!("{}/{}", policy_ns, meta.name.as_deref().unwrap_or(""));
        let targets: Vec<PolicyTargetKey> = evaluation.target.iter().cloned().collect();
        super::resolve_policy_ancestor_gateways_for_targets(&targets, &policy_key, ctx)
            .into_keys()
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
    fn is_ca_resolved(spec: &BackendTLSPolicySpec, name: &str) -> bool {
        spec.resolved_ca_certificates.as_deref().is_some_and(|secrets| {
            secrets.iter().any(|secret| {
                secret.metadata.name.as_deref() == Some(name)
                    && secret
                        .data
                        .as_ref()
                        .and_then(|data| data.get(CA_CERT))
                        .is_some_and(|value| !value.0.is_empty())
            })
        })
    }

    #[inline]
    fn secret_has_client_cert_pair(secret: &k8s_openapi::api::core::v1::Secret) -> bool {
        secret.data.as_ref().is_some_and(|data| {
            data.get(CERT).is_some_and(|value| !value.0.is_empty())
                && data.get(KEY).is_some_and(|value| !value.0.is_empty())
        })
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<BackendTLSPolicy> for BackendTlsPolicyHandler {
    fn finalize_transport(&self, policy: &mut BackendTLSPolicy, _ctx: &HandlerContext) {
        filter_current_native_attachment_status(policy, &self.controller_name);
    }

    fn validate(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) -> Vec<String> {
        self.evaluate_target(&btp.metadata, &btp.spec, ctx).errors
    }

    async fn parse(&self, btp: BackendTLSPolicy, ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        self.parse_policy(btp, ctx).await
    }

    async fn on_change(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        self.on_change_policy(&btp.metadata, &btp.spec.target_refs, ctx).await;
    }

    async fn on_delete(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        self.on_delete_policy(&btp.metadata, &btp.spec.target_refs, ctx).await;
    }

    fn update_status(&self, btp: &mut BackendTLSPolicy, ctx: &HandlerContext, validation_errors: &[String]) {
        self.update_policy_status(&btp.metadata, &btp.spec, &mut btp.status, ctx, validation_errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use chrono::{DateTime, Utc};
    use edgion_resources::resources::backend_tls_policy::{
        BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation, WellKnownCACertificates,
    };
    use edgion_resources::ResourceKind;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
    use kube::api::ObjectMeta;
    use std::sync::Arc;

    // -------- should_requeue_siblings (pure decision) --------

    #[test]
    fn should_requeue_when_target_set_changes() {
        assert!(should_requeue_siblings(
            &["ns/svc-a".to_string()],
            &["ns/svc-b".to_string()],
            Some(dt("2024-01-01T00:00:00+00:00")),
            Some(dt("2024-01-01T00:00:00+00:00")),
        ));
    }

    #[test]
    fn should_requeue_when_timestamp_changes_with_same_targets() {
        // BTP delete+recreated within one PollWatcher tick — same name and
        // targets, but a new creation_timestamp must trigger sibling re-evaluation.
        let targets = ["ns/my-svc".to_string()];
        assert!(should_requeue_siblings(
            &targets,
            &targets,
            Some(dt("2024-01-01T00:00:00+00:00")),
            Some(dt("2024-06-01T00:00:00+00:00")),
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
            Some(dt("2024-01-01T00:00:00+00:00")),
            Some(dt("2024-01-01T00:00:00+00:00")),
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
            Some(dt("2024-01-01T00:00:00+00:00")),
        ));
    }

    // -------- parse_policy delete+recreate path (state-level) --------

    fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        (ctx, runtime)
    }

    /// Parse an RFC3339 literal into the `DateTime<Utc>` the precedence store holds.
    fn dt(s: &str) -> DateTime<Utc> {
        s.parse().expect("valid RFC3339")
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
                current_status: None,
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
                resolved_target_class: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: false,
            },
            status: None,
        }
    }

    fn install_backend_processor(
        runtime: &Arc<ControllerRuntime>,
        backend: EdgionBackend,
    ) -> (
        Arc<crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor<EdgionBackend>>,
        Arc<crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore<EdgionBackend>>,
    ) {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;

        let processor = Arc::new(ResourceProcessor::<EdgionBackend>::new(
            "EdgionBackend",
            16,
            Arc::new(EdgionBackendHandler::new()),
            runtime.clone(),
        ));
        let key = format!(
            "{}/{}",
            backend.metadata.namespace.as_deref().unwrap_or("default"),
            backend.metadata.name.as_deref().unwrap_or_default()
        );
        let raw = Arc::new(crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore::new());
        raw.upsert(key, Arc::new(backend.clone()));
        processor.set_raw_store(raw.clone());
        processor.save(backend, None);
        runtime.processor_registry.register(processor.clone());
        (processor, raw)
    }

    #[tokio::test]
    async fn ai_target_publishes_typed_marker_and_dependency() {
        let (ctx, runtime) = make_ctx();
        let backend: EdgionBackend = serde_json::from_value(serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackend",
            "metadata": {"name": "models", "namespace": "ns"},
            "spec": {"ai": {
                "provider": "OpenAI",
                "credentialPool": {"credentials": [{
                    "name": "primary", "secretRef": {"name": "provider-key"}
                }]},
                "models": [{"name": "model-a"}]
            }}
        }))
        .unwrap();
        install_backend_processor(&runtime, backend);
        let handler = BackendTlsPolicyHandler::default();
        let mut policy = make_btp("ns", "ai-tls", "models", "2026-01-01T00:00:00+00:00");
        policy.spec.target_refs[0].group = "edgion.io".to_string();
        policy.spec.target_refs[0].kind = "EdgionBackend".to_string();

        let ProcessResult::Continue(parsed) = handler.parse_policy(policy, &ctx).await else {
            panic!("policy should continue");
        };
        assert_eq!(parsed.spec.resolved_target_class, Some(ResolvedPolicyTargetClass::Ai));
        let target = parsed.spec.target_refs[0].canonical_key("ns").unwrap();
        assert_eq!(
            runtime.backend_ref_mgr.get_dependencies("BackendTLSPolicy/ns/ai-tls"),
            vec![target.to_string()]
        );
        assert_eq!(
            runtime
                .backend_tls_policy
                .target_mgr()
                .get_dependencies("BackendTLSPolicy/ns/ai-tls"),
            vec![target.to_string()]
        );
    }

    #[test]
    fn ai_target_rejects_section_and_wrong_variant_identity() {
        let (ctx, _) = make_ctx();
        let handler = BackendTlsPolicyHandler::default();
        let mut policy = make_btp("ns", "ai-tls", "models", "2026-01-01T00:00:00+00:00");
        policy.spec.target_refs[0].group = "edgion.io".to_string();
        policy.spec.target_refs[0].kind = "EdgionBackend".to_string();
        policy.spec.target_refs[0].section_name = Some("unused".to_string());
        assert!(handler
            .validate(&policy, &ctx)
            .iter()
            .any(|error| error.contains("sectionName is not supported")));

        policy.spec.target_refs[0].section_name = None;
        policy.spec.target_refs[0].group = "example.io".to_string();
        assert!(handler
            .validate(&policy, &ctx)
            .iter()
            .any(|error| error.contains("supported targets")));
    }

    #[test]
    fn direct_ca_namespace_and_duplicate_targets_are_rejected() {
        let (ctx, _) = make_ctx();
        let handler = BackendTlsPolicyHandler::default();
        let mut policy = make_btp("ns", "tls", "svc", "2026-01-01T00:00:00+00:00");
        policy.spec.validation.well_known_ca_certificates = None;
        policy.spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
            group: String::new(),
            kind: "Secret".to_string(),
            name: "ca".to_string(),
            namespace: Some("other".to_string()),
        }]);
        policy.spec.target_refs.push(policy.spec.target_refs[0].clone());
        let errors = handler.validate(&policy, &ctx);
        assert!(errors.iter().any(|error| error.contains("exactly one")));
        assert!(errors.iter().any(|error| error.contains("must not specify namespace")));
    }

    #[tokio::test]
    async fn unsupported_well_known_ca_gets_invalid_status_and_no_runtime_target() {
        let (ctx, runtime) = make_ctx();
        let handler = BackendTlsPolicyHandler::default();
        runtime.service_ref_mgr.add_ref(
            "ns/svc".to_string(),
            ResourceRef::new(ResourceKind::HTTPRoute, Some("ns".to_string()), "route".to_string()),
        );
        runtime.gateway_route_index.update_route(
            ResourceKind::HTTPRoute,
            "ns/route",
            &[ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some("ns".to_string()),
                name: "gateway".to_string(),
                section_name: None,
                port: None,
            }],
            "ns",
        );
        let mut policy = make_btp("ns", "tls", "svc", "2026-01-01T00:00:00+00:00");
        policy.spec.validation.well_known_ca_certificates =
            Some(WellKnownCACertificates::Other("example.com/Custom".to_string()));
        let errors = handler.validate(&policy, &ctx);
        assert!(errors.iter().any(|error| error.contains("only System")));
        let ProcessResult::Continue(mut rejected) = handler.parse(policy, &ctx).await else {
            panic!("unsupported CA name must remain available for status");
        };
        assert!(!rejected.spec.use_system_ca);
        assert!(rejected.spec.resolved_target_class.is_none());
        handler.update_status(&mut rejected, &ctx, &errors);
        assert!(rejected.status.as_ref().unwrap().ancestors.iter().any(|ancestor| {
            ancestor.conditions.iter().any(|condition| {
                condition.type_ == "Accepted"
                    && condition.status == "False"
                    && condition.message.contains("only System")
            })
        }));

        rejected.spec.validation.well_known_ca_certificates = Some(WellKnownCACertificates::System);
        let errors = handler.validate(&rejected, &ctx);
        assert!(errors.is_empty());
        let ProcessResult::Continue(mut corrected) = handler.parse(rejected, &ctx).await else {
            panic!("corrected CA name must parse");
        };
        assert!(corrected.spec.use_system_ca);
        handler.update_status(&mut corrected, &ctx, &errors);
        assert!(corrected.status.as_ref().unwrap().ancestors.iter().any(|ancestor| {
            ancestor
                .conditions
                .iter()
                .any(|condition| condition.type_ == "Accepted" && condition.status == "True")
        }));
    }

    #[test]
    fn status_rebuild_preserves_foreign_controller_ancestors() {
        let (ctx, _) = make_ctx();
        let handler = BackendTlsPolicyHandler::default();
        let mut policy = make_btp("ns", "tls", "svc", "2026-01-01T00:00:00+00:00");
        let foreign = PolicyAncestorStatus {
            ancestor_ref: ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some("ns".to_string()),
                name: "foreign".to_string(),
                section_name: None,
                port: None,
            },
            controller_name: "example.io/other-controller".to_string(),
            conditions: Vec::new(),
        };
        policy.status = Some(BackendTLSPolicyStatus {
            ancestors: vec![foreign.clone()],
        });

        handler.update_policy_status(&policy.metadata, &policy.spec, &mut policy.status, &ctx, &[]);

        assert!(policy
            .status
            .unwrap()
            .ancestors
            .iter()
            .any(|ancestor| ancestor.controller_name == foreign.controller_name));
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

        let target_key = make_target_conflict_key(&service_target_key("ns", "my-svc"), None);
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
            old_ts_before_recreate,
            new_ts_after_recreate,
        ));
    }

    /// Locks the coalescing guarantee for BackendTLSPolicy -> EdgionBackendTrafficPolicy:
    /// a delete->recreate that retargets the policy to a different Service must
    /// requeue the EBTPs bound to the DROPPED target Service, even though
    /// `on_delete_policy` never runs. `on_change_policy`'s `requeue_dependent_ebtps`
    /// only sees the current targetRefs, so the dropped target's EBTPs rely on the
    /// return-delta requeue added to `parse_policy`.
    #[tokio::test]
    async fn parse_policy_requeues_dropped_target_ebtps_on_retarget() {
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;

        struct EbtpNoop;
        #[async_trait::async_trait]
        impl ProcessorHandler<EdgionBackendTrafficPolicy> for EbtpNoop {}

        let runtime = Arc::new(ControllerRuntime::default());

        // Register an EBTP processor so ctx.requeue("EdgionBackendTrafficPolicy", ...)
        // has an observable queue.
        let ebtp_processor = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
            "EdgionBackendTrafficPolicy",
            100,
            Arc::new(EbtpNoop),
            runtime.clone(),
        ));
        runtime.processor_registry.register(ebtp_processor.clone());

        // An EBTP depends on ns/svc-a, registered under its typed Service key.
        let ebtp_ref = ResourceRef::new(
            ResourceKind::EdgionBackendTrafficPolicy,
            Some("ns".to_string()),
            "ebtp-a".to_string(),
        );
        runtime
            .edgion_backend_traffic_policy
            .target_mgr()
            .add_ref(service_target_key("ns", "svc-a").to_string(), ebtp_ref.clone());

        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        let handler = BackendTlsPolicyHandler::default();

        // Generation 1: BTP targets svc-a.
        let _ = handler
            .parse_policy(make_btp("ns", "btp1", "svc-a", "2024-01-01T00:00:00+00:00"), &ctx)
            .await;
        while tokio::time::timeout(
            std::time::Duration::from_millis(500),
            ebtp_processor.workqueue().dequeue(),
        )
        .await
        .is_ok()
        {}

        // Coalesced delete->recreate: the worker only sees generation 2, now
        // targeting svc-b (svc-a dropped). on_delete_policy never runs. The EBTP
        // on the dropped svc-a must still be requeued.
        let _ = handler
            .parse_policy(make_btp("ns", "btp1", "svc-b", "2024-02-01T00:00:00+00:00"), &ctx)
            .await;

        let mut seen = std::collections::HashSet::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline && !seen.contains("ns/ebtp-a") {
            if let Ok(Some(item)) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                ebtp_processor.workqueue().dequeue(),
            )
            .await
            {
                seen.insert(item.key.clone());
                ebtp_processor.workqueue().done(&item.key);
            }
        }
        assert!(
            seen.contains("ns/ebtp-a"),
            "EBTP on dropped target svc-a was not requeued; seen: {:?}",
            seen
        );
    }

    #[test]
    fn get_siblings_returns_other_policies_targeting_same_service() {
        let runtime = ControllerRuntime::default();
        let target_key = make_target_conflict_key(&service_target_key("ns", "my-svc"), None);

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

    // -------- Critical 2 regression: BackendTLSPolicy change/delete requeues the
    // -------- dependent EdgionBackendTrafficPolicy --------

    /// Build a minimal, spec-locally-valid EdgionBackendTrafficPolicy targeting
    /// `namespace/service` and register it into the target-conflict index via
    /// `EdgionBackendTrafficPolicyHandler::parse` — mirroring what a real EBTP reconcile does
    /// (`edgion_backend_traffic_policy.rs::parse` registers a valid spec under
    /// the typed `PolicyTargetKey` identity).
    async fn register_ebtp_target(namespace: &str, name: &str, service: &str, ctx: &HandlerContext) {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler;
        use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
        use serde_json::json;

        let ebtp: EdgionBackendTrafficPolicy = serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionBackendTrafficPolicy",
            "metadata": { "name": name, "namespace": namespace },
            "spec": { "targetRefs": [ { "kind": "Service", "name": service } ] },
        }))
        .expect("valid EdgionBackendTrafficPolicy JSON");
        let _ = EdgionBackendTrafficPolicyHandler::new("test.edgion.io/controller".to_string())
            .parse(ebtp, ctx)
            .await;
    }

    /// CRITICAL regression test: applying/changing a BackendTLSPolicy must requeue every
    /// EdgionBackendTrafficPolicy that targets the same Service. Without this edge, the EBTP
    /// `upstreamAuthority` cross-object admission rule — which runs only inside EBTP's own
    /// `validate()` — is order-dependent: applying the offending BackendTLSPolicy AFTER the
    /// EBTP already exists (the canonical ops sequence this rule is meant to catch) would
    /// otherwise never re-trigger the EBTP's reconcile, leaving a stale `Accepted=True` forever.
    #[tokio::test]
    async fn backend_tls_policy_change_requeues_dependent_ebtp() {
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
        use std::sync::atomic::Ordering;

        let (ctx, rt) = make_ctx();

        // Register an EdgionBackendTrafficPolicy processor so `ctx.requeue("EdgionBackendTrafficPolicy", ..)`
        // has somewhere to land; its workqueue metrics are the test's observation point.
        let ebtp_proc = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
            "EdgionBackendTrafficPolicy",
            128,
            Arc::new(
                crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler::new(
                    "test.edgion.io/controller".to_string(),
                ),
            ),
            rt.clone(),
        ));
        rt.processor_registry.register(ebtp_proc.clone());

        register_ebtp_target("ns", "ebtp", "payments", &ctx).await;

        assert_eq!(
            ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
            0,
            "sanity: nothing requeued yet"
        );

        // Apply a BackendTLSPolicy targeting the same Service, THEN drive on_change (mirrors
        // the reconcile pipeline: parse() followed unconditionally by on_change()).
        let handler = BackendTlsPolicyHandler::default();
        let btp = make_btp("ns", "tls", "payments", "2026-01-01T00:00:00+00:00");
        let _ = handler.parse(btp.clone(), &ctx).await;
        handler.on_change(&btp, &ctx).await;

        assert_eq!(
            ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
            1,
            "BackendTLSPolicy on_change must requeue the dependent EdgionBackendTrafficPolicy"
        );
    }

    /// CRITICAL regression test: deleting a BackendTLSPolicy must requeue every
    /// EdgionBackendTrafficPolicy that targeted the same Service, otherwise a stale
    /// `Accepted=False` (from the now-removed offending hostname) outlives the object that
    /// caused it.
    #[tokio::test]
    async fn backend_tls_policy_delete_requeues_dependent_ebtp() {
        use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
        use edgion_resources::prelude_resources::EdgionBackendTrafficPolicy;
        use std::sync::atomic::Ordering;

        let (ctx, rt) = make_ctx();

        let ebtp_proc = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
            "EdgionBackendTrafficPolicy",
            128,
            Arc::new(
                crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionBackendTrafficPolicyHandler::new(
                    "test.edgion.io/controller".to_string(),
                ),
            ),
            rt.clone(),
        ));
        rt.processor_registry.register(ebtp_proc.clone());

        register_ebtp_target("ns", "ebtp", "payments", &ctx).await;

        let handler = BackendTlsPolicyHandler::default();
        let btp = make_btp("ns", "tls", "payments", "2026-01-01T00:00:00+00:00");
        let _ = handler.parse(btp.clone(), &ctx).await;

        assert_eq!(
            ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
            0,
            "sanity: parse() alone (no on_delete yet) must not requeue the EBTP"
        );

        handler.on_delete(&btp, &ctx).await;

        assert_eq!(
            ebtp_proc.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
            1,
            "BackendTLSPolicy on_delete must requeue the dependent EdgionBackendTrafficPolicy"
        );
    }
}
