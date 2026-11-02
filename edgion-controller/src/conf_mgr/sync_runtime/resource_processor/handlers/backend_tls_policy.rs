//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources with Gateway API standard status management.

use super::super::credential::pipeline::{fetch_bound_configmap, fetch_bound_secret, BoundFetchOutcome};
use edgion_resources::resources::refs::backend_tls_fields::{BackendTlsRefs, MAX_BACKEND_TLS_CA_REFERENCES};
use edgion_resources::resources::refs::policy_fields::BackendTlsPolicyTargets;
use std::collections::BTreeSet;

use k8s_openapi::api::core::v1::Service;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::conf_mgr::sync_runtime::resource_processor::backend_tls_policy_ref::make_target_conflict_key;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::backend_capability::classify_raw_backend;
use crate::conf_mgr::sync_runtime::resource_processor::status_pipeline::filter_current_native_attachment_status;
use crate::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler, ResourceRef,
};
use edgion_resources::constants::secret_keys::tls::CA_CERT;
use edgion_resources::prelude_resources::BackendTLSPolicy;
#[cfg(test)]
use edgion_resources::prelude_resources::EdgionBackend;
use edgion_resources::resources::backend_tls_policy::{
    parse_client_certificate_option, BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyStatus,
};
use edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence;
use edgion_resources::resources::common::{ParentReference, PolicyAncestorStatus};
use edgion_resources::resources::edgion_backend_traffic_policy::{
    PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass,
};
use edgion_resources::ResourceKind;
use k8s_openapi::jiff::Timestamp;

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
    old_timestamp: Option<Timestamp>,
    new_timestamp: Option<Timestamp>,
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
async fn requeue_dependent_ebtps(policy: &BackendTLSPolicy, ctx: &HandlerContext) {
    for field in BackendTlsPolicyTargets::from_policy(policy).fields() {
        let Ok(target_key) = field.canonical_key() else {
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

        if validation
            .ca_certificate_refs
            .as_ref()
            .is_some_and(|refs| refs.len() > MAX_BACKEND_TLS_CA_REFERENCES)
        {
            errors.push("caCertificateRefs must contain at most 8 references".to_string());
            return errors;
        }

        if let Some(refs) = &validation.ca_certificate_refs {
            let mut seen_refs = BTreeSet::new();
            for ca_ref in refs {
                let canonical_group = ca_ref.group.as_str();
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

    fn evaluate_target(&self, policy: &BackendTLSPolicy, ctx: &HandlerContext) -> PolicyTargetEvaluation {
        let spec = &policy.spec;
        let mut evaluation = PolicyTargetEvaluation {
            errors: self.validate_policy(spec),
            ..PolicyTargetEvaluation::default()
        };
        if spec.target_refs.len() != 1 {
            return evaluation;
        }

        let field = BackendTlsPolicyTargets::from_policy(policy).fields().next();
        let Some(field) = field else { return evaluation };
        let target = match field.canonical_key() {
            Ok(target) => target,
            Err(error) => {
                evaluation.errors.push(error);
                return evaluation;
            }
        };
        if target.target_kind == PolicyTargetKind::EdgionBackend && field.section_name().is_some() {
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

    fn all_runtime_refs_resolved(policy: &BackendTLSPolicy) -> bool {
        BackendTlsRefs::from_policy(policy).all_materials_resolved()
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
        target_manager.clear_value_refs(&resource_ref);
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_value_refs(&resource_ref);
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);

        btp.spec.resolved_target_class = None;
        btp.spec.prepared_ancestor_admission = None;
        btp.spec.prepared_precedence = Some(PreparedPolicyPrecedence {
            creation_timestamp: btp.metadata.creation_timestamp.as_ref().map(|time| time.0),
        });
        btp.spec.resolved_ca_certificates = None;
        btp.spec.resolved_client_certificate = None;

        // Register a syntactically canonical target dependency before checking
        // existence. Creation of a currently-missing target must wake this policy.
        for target in BackendTlsPolicyTargets::from_policy(&btp)
            .fields()
            .filter_map(|field| field.canonical_key().ok())
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

        // Borrow the actual owner while acquiring; write material only after all loans end.
        let (resolved_ca_secrets, resolved_client) = {
            let fields = BackendTlsRefs::from_policy(&btp);
            let mut ca = Vec::new();
            let mut client = None;
            if fields.bounded_materials() {
                for ordinal in 0..fields.ca_count() {
                    let Some(field) = fields.ca(ordinal) else {
                        continue;
                    };
                    let token = field.token();
                    let Ok(reference) = token.validated() else {
                        continue;
                    };
                    match reference.target().group_kind().kind() {
                        "Secret" => {
                            if let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, &resource_ref, token) {
                                ca.push(super::super::credential::ca_refs::ordinary_secret_material(
                                    authorized.secret,
                                ));
                            }
                        }
                        "ConfigMap" => {
                            if let BoundFetchOutcome::Ok(configmap) = fetch_bound_configmap(ctx, &resource_ref, token) {
                                if let Some(material) =
                                    super::super::credential::ca_refs::configmap_ca_material(&configmap)
                                {
                                    ca.push(material);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Ok(Some(field)) = fields.client() {
                    if let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, &resource_ref, field.token()) {
                        client = Some(super::super::credential::ca_refs::ordinary_secret_material(
                            authorized.secret,
                        ));
                    }
                }
            }
            (ca, client)
        };
        btp.spec.resolved_ca_certificates = (!resolved_ca_secrets.is_empty()).then_some(resolved_ca_secrets);
        btp.spec.resolved_client_certificate = resolved_client;

        let evaluation = self.evaluate_target(&btp, ctx);
        let mut new_target_keys = Vec::new();
        if evaluation.target_class.is_some() && Self::all_runtime_refs_resolved(&btp) {
            if let Some(target) = &evaluation.target {
                let section = btp.spec.target_refs[0].section_name.as_deref();
                let conflict_key = make_target_conflict_key(target, section);
                target_manager.add_ref(conflict_key.clone(), resource_ref.clone());
                new_target_keys.push(conflict_key);
                btp.spec.resolved_target_class = evaluation.target_class;
            }
        }
        new_target_keys.sort();

        let new_timestamp = btp
            .spec
            .prepared_precedence
            .and_then(|prepared| prepared.creation_timestamp);
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

    async fn on_delete_policy(&self, policy: &BackendTLSPolicy, ctx: &HandlerContext) {
        let meta = &policy.metadata;
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
            .clear_value_refs(&resource_ref);
        ctx.runtime()
            .backend_tls_policy
            .remove_timestamp(&resource_ref.resource_key());

        // Clear target and Secret dependency edges.
        ctx.runtime().service_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().backend_ref_mgr.clear_value_refs(&resource_ref);
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);

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
        requeue_dependent_ebtps(policy, ctx).await;
    }

    fn update_policy_status(
        &self,
        policy: &BackendTLSPolicy,
        status: &mut Option<BackendTLSPolicyStatus>,
        ctx: &HandlerContext,
        validation_errors: &[String],
    ) {
        let meta = &policy.metadata;
        let spec = &policy.spec;
        let generation = meta.generation;
        let evaluation = self.evaluate_target(policy, ctx);
        let mut target_errors = validation_errors.to_vec();
        target_errors.extend(evaluation.errors.iter().cloned());
        target_errors.sort();
        target_errors.dedup();

        // --- Classify CA reference errors with precise reasons ---
        let mut kind_errors = Vec::new(); // → reason: InvalidKind
        let mut ref_errors = Vec::new(); // → reason: InvalidCACertificateRef
        let mut client_cert_errors = Vec::new();

        let material_fields = BackendTlsRefs::from_policy(policy);
        let bounded_materials = material_fields.bounded_materials();
        if !bounded_materials {
            ref_errors.push("CA reference/material cardinality is invalid".to_string());
        } else if !spec.use_system_ca {
            if let Some(refs) = &spec.validation.ca_certificate_refs {
                for (ordinal, ca_ref) in refs.iter().enumerate() {
                    if Self::is_secret_ref(ca_ref) || Self::is_configmap_ref(ca_ref) {
                        if !material_fields.ca(ordinal).is_some_and(|field| field.resolved()) {
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

        match material_fields.client() {
            Ok(Some(field)) if !field.resolved() => client_cert_errors
                .push("Client certificate reference is unresolved or has invalid material".to_string()),
            Ok(None) if spec.resolved_client_certificate.is_some() => {
                client_cert_errors.push("Client certificate material has no declared reference".to_string())
            }
            Err(_) => client_cert_errors.push("Invalid client certificate reference or material bounds".to_string()),
            _ => {}
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
            total_ca_refs > 0 && (!bounded_materials || (kind_errors.len() + ref_errors.len()) >= total_ca_refs)
        } else {
            false
        };

        // --- Conflict detection ---
        let is_conflicted = if target_errors.is_empty() && Self::all_runtime_refs_resolved(policy) {
            let policy_key = ResourceRef::new(
                ResourceKind::BackendTLSPolicy,
                meta.namespace.clone(),
                meta.name.clone().unwrap_or_default(),
            )
            .resource_key();
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
        if ctx.runtime().status_identity().is_none() {
            super::super::policy_ancestor_capacity::compose_native(
                meta.namespace.as_deref().unwrap_or("default"),
                &self.controller_name,
                &old_ancestors,
                &mut status.ancestors,
            );
        }
    }

    async fn on_change_policy(&self, policy: &BackendTLSPolicy, ctx: &HandlerContext) {
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
        requeue_dependent_ebtps(policy, ctx).await;
    }

    /// Resolve the Gateway ancestors for a BackendTLSPolicy.
    ///
    /// Chain: BTP.targetRef(Service) -> ServiceRefManager(service->routes) -> GatewayRouteIndex(route->gateways)
    ///
    /// Returns every deduplicated, sorted Gateway candidate. The final ownership
    /// composer applies the 16-entry budget after accounting for protected history.
    fn resolve_ancestor_gateways(
        meta: &ObjectMeta,
        evaluation: &PolicyTargetEvaluation,
        ctx: &HandlerContext,
    ) -> Vec<(String, String)> {
        let policy_key = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            meta.namespace.clone(),
            meta.name.clone().unwrap_or_default(),
        )
        .resource_key();
        let targets: Vec<PolicyTargetKey> = evaluation.target.iter().cloned().collect();
        super::resolve_policy_ancestor_gateways_for_targets(&targets, &policy_key, ctx)
            .into_keys()
            .filter_map(|k| k.split_once('/').map(|(ns, name)| (ns.to_string(), name.to_string())))
            .collect()
    }

    #[inline]
    fn is_secret_ref(ca_ref: &BackendTLSPolicyCACertificateRef) -> bool {
        ca_ref.group.is_empty() && ca_ref.kind == "Secret"
    }

    #[inline]
    fn is_configmap_ref(ca_ref: &BackendTLSPolicyCACertificateRef) -> bool {
        ca_ref.group.is_empty() && ca_ref.kind == "ConfigMap"
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<BackendTLSPolicy> for BackendTlsPolicyHandler {
    fn finalize_transport(&self, policy: &mut BackendTLSPolicy, _ctx: &HandlerContext) {
        filter_current_native_attachment_status(policy, &self.controller_name);
    }

    fn validate(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) -> Vec<String> {
        self.evaluate_target(btp, ctx).errors
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
        let mut status = btp.status.take();
        self.update_policy_status(btp, &mut status, ctx, validation_errors);
        btp.status = status;
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/support/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs"]
mod test_support;
#[cfg(test)]
use test_support::*;
