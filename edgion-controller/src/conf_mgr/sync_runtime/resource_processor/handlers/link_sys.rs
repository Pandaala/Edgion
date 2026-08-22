//! LinkSys Handler
//!
//! Handles LinkSys resources with:
//! - Validation (preparse hook)
//! - SecretRef resolution via `resolve_secret` pipeline (attaches Secret to spec)
//! - SecretRefManager registration for cascading updates
//! - Gateway API standard status management
//!
//! Note: the resolved Secret is embedded into the corresponding `*Auth.secret` field,
//! which is serialized across conf_sync to the Gateway. Material parsing happens on
//! the Gateway side via `edgion_resources::credentials`.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_request_secrets, resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, resolved_refs_condition, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::prelude_resources::LinkSys;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::link_sys::webhook::{ResolvedSecrets, WebhookServiceConfig};
use edgion_resources::resources::link_sys::{LinkSysStatus, SystemConfig};

use super::auth_spec::{AuthSpec, TlsSpec};
use super::san_guard;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Secret;

/// LinkSys handler
pub struct LinkSysHandler;

impl LinkSysHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinkSysHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate a pipeline `ResolveOutcome<T>` into `Result<T, ResolvedRefsError>`.
///
/// Cross-namespace denial maps to the Gateway-API standard `RefNotPermitted`
/// (not a dedicated `ReferenceGrantMissing` reason): the pipeline cannot
/// distinguish "no grant yet" from "grant denies", so a more specific reason
/// would promise operators a signal we cannot deliver.
fn outcome_to_result<T>(outcome: ResolveOutcome<T>) -> Result<T, ResolvedRefsError> {
    match outcome {
        ResolveOutcome::Ok(t) => Ok(t),
        ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            Err(ResolvedRefsError::SecretNotFound { namespace, name })
        }
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => Err(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns,
            from_kind: ResourceKind::LinkSys.as_str(),
            target_namespace: to_ns,
            target_name: secret_name,
        }),
        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)) => {
            Err(ResolvedRefsError::MissingKeys { keys })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }) => {
            Err(ResolvedRefsError::DecodeError { key, detail })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidValue { key, class }) => {
            Err(ResolvedRefsError::DecodeError {
                key,
                detail: class.to_string(),
            })
        }
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }) => {
            Err(ResolvedRefsError::InvalidSecretType { got, allowed })
        }
    }
}

/// Build a `ResolvedRefs` condition reflecting the outcome of
/// `resolve_for_spec` and merge it into `ls.status.conditions`.
///
/// Delegates reason/message formatting to `status_utils::resolved_refs_condition`
/// so all credential-backed CRDs produce the same wording. This MUST run even
/// on success so the condition transitions `False → True` when an operator
/// fixes the Secret.
fn apply_resolved_refs_condition(ls: &mut LinkSys, generation: Option<i64>, errors: &[ResolvedRefsError]) {
    let status = ls.status.get_or_insert_with(LinkSysStatus::default);
    update_condition(&mut status.conditions, resolved_refs_condition(errors, generation));
}

/// Does this resolution error require the operator to edit Secret *content*
/// (keys / value format / Secret type) to recover, as opposed to just
/// creating a missing Secret or ReferenceGrant?
///
/// - `false` (info-level log) — the Secret or ReferenceGrant is simply absent
///   or denied. Reprocessing will pick up the fix once the missing resource
///   arrives; no log volume amplification needed.
/// - `true` (warn-level log) — the Secret exists but its content is unusable.
///   Re-queues without operator edits won't help, so louder logging is
///   justified. `RefNotPermitted` is intentionally *not* in this bucket even
///   though "operator forgot Grant" is an edit-to-fix case, because the
///   pipeline cannot distinguish "forgot to create Grant" from "deliberately
///   denied".
fn needs_operator_secret_edit(err: &ResolvedRefsError) -> bool {
    matches!(
        err,
        ResolvedRefsError::MissingKeys { .. }
            | ResolvedRefsError::DecodeError { .. }
            | ResolvedRefsError::InvalidSecretType { .. }
    )
}

#[async_trait::async_trait]
impl ProcessorHandler<LinkSys> for LinkSysHandler {
    fn preparse(&self, ls: &mut LinkSys, _ctx: &HandlerContext) -> Vec<String> {
        if !ls.spec.unknown_fields.is_empty() {
            let paths = bounded_unknown_field_paths(&ls.spec.unknown_fields);
            tracing::warn!(
                component = "conf_mgr",
                event = "unknown_config_fields_detected",
                kind = "LinkSys",
                namespace = %ls.metadata.namespace.as_deref().unwrap_or("default"),
                name = %ls.metadata.name.as_deref().unwrap_or(""),
                paths = ?paths,
                "LinkSys document has keys the typed schema does not recognize; they are ignored and config still syncs"
            );
        }
        let mut errors = ls.validate_config();
        // Phase 1 guard: reject outbound TLS subjectAltNames/hostname on LinkSys
        // providers where SAN pinning enforcement is not yet implemented.
        errors.extend(san_guard::check_link_sys_san(ls));
        errors
    }

    async fn parse(&self, mut ls: LinkSys, ctx: &HandlerContext) -> ProcessResult<LinkSys> {
        let owner = ResourceRef::new(
            ResourceKind::LinkSys,
            ls.metadata.namespace.clone(),
            ls.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old Secret references first (for update scenario) so that a Secret
        // that was previously referenced but is no longer referenced after an update
        // does not trigger spurious requeues.
        ctx.secret_ref_manager().clear_resource_refs(&owner);

        let errors = resolve_for_spec(ctx, &owner, &mut ls.spec.config).await;
        let generation = ls.metadata.generation;
        apply_resolved_refs_condition(&mut ls, generation, &errors);

        for err in &errors {
            if needs_operator_secret_edit(err) {
                // Invalid Secret content: requeues without Secret edits won't
                // help. The spec is still pushed to the Gateway via conf_sync
                // with `auth.secret` / `tls.certs.secret` left as `None`
                // (partially resolved when only one side failed). Gateway
                // providers must check `CredentialState::is_buildable` and
                // refuse to build a client on `Pending` / `Invalid` material;
                // see `edgion_common::link_sys_core::credential` for the contract.
                tracing::warn!(
                    link_sys = %owner.key(),
                    ?err,
                    "Credential resolution failed (invalid Secret data)"
                );
            } else {
                tracing::info!(
                    link_sys = %owner.key(),
                    ?err,
                    "Credential resolution pending, will reprocess when Secret/ReferenceGrant arrives"
                );
            }
        }

        // Continue either way so `update_status` persists `ResolvedRefs` and
        // the resource stays alive for Secret/Grant wakeups.
        ProcessResult::Continue(ls)
    }

    async fn on_delete(&self, ls: &LinkSys, ctx: &HandlerContext) {
        let owner = ResourceRef::new(
            ResourceKind::LinkSys,
            ls.metadata.namespace.clone(),
            ls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&owner);
        tracing::debug!(
            link_sys = %owner.key(),
            "Cleared SecretRefManager references on LinkSys delete"
        );
    }

    fn update_status(&self, ls: &mut LinkSys, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ls.metadata.generation;

        // `ResolvedRefs` is maintained by `parse` (see `apply_resolved_refs_condition`);
        // this method owns `Accepted` and the independent unknown-field advisory.
        let status = ls.status.get_or_insert_with(LinkSysStatus::default);

        if validation_errors.is_empty() {
            update_condition(&mut status.conditions, accepted_condition(generation));
        } else {
            update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ),
            );
        }
        super::super::unknown_fields_stamp::apply_unknown_fields_condition(
            &mut status.conditions,
            &ls.spec.unknown_fields,
            generation,
        );
    }
}

#[cfg(test)]
mod unknown_fields_tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
        condition_reasons, condition_true, condition_types,
    };
    use std::sync::Arc;

    fn resource(paths: Vec<String>) -> LinkSys {
        let spec = serde_json::from_value(serde_json::json!({
            "type": "redis",
            "config": {"endpoints": ["redis://127.0.0.1:6379"]}
        }))
        .expect("minimal LinkSys spec");
        let mut resource = LinkSys::new("redis", spec);
        resource.metadata.namespace = Some("default".to_string());
        resource.metadata.generation = Some(7);
        resource.spec.unknown_fields = paths;
        resource
    }

    #[test]
    fn update_status_adds_and_clears_advisory_without_disturbing_other_conditions() {
        let handler = LinkSysHandler::new();
        let mut resource = resource(vec!["config.timeout.conect".to_string()]);
        resource
            .status
            .get_or_insert_with(LinkSysStatus::default)
            .conditions
            .push(resolved_refs_condition(&[], Some(7)));
        let ctx = HandlerContext::new(
            None,
            None,
            Default::default(),
            5,
            20,
            Arc::new(crate::conf_mgr::ControllerRuntime::default()),
        );

        handler.update_status(&mut resource, &ctx, &[]);
        let conditions = &resource.status.as_ref().expect("status").conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == condition_types::UNKNOWN_CONFIG_FIELDS
                && condition.reason == condition_reasons::UNKNOWN_FIELDS_DETECTED
                && condition.status == "True"
        }));
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == condition_types::ACCEPTED && condition.status == "True"));
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == condition_types::RESOLVED_REFS && condition.status == "True"));

        resource.spec.unknown_fields.clear();
        resource
            .status
            .as_mut()
            .expect("status")
            .conditions
            .push(condition_true("Unrelated", "Stable", "keep", Some(7)));
        handler.update_status(&mut resource, &ctx, &[]);
        let conditions = &resource.status.as_ref().expect("status").conditions;
        assert!(conditions
            .iter()
            .all(|condition| condition.type_ != condition_types::UNKNOWN_CONFIG_FIELDS));
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == condition_types::RESOLVED_REFS && condition.status == "True"));
        assert!(conditions.iter().any(|condition| condition.type_ == "Unrelated"));
    }
}

// ============================================================================
// Helper functions: resolve Secret per auth variant and inject into spec
// ============================================================================

async fn drive_standard<A: AuthSpec, T: TlsSpec>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    auth: Option<&mut A>,
    tls: Option<&mut T>,
) -> Vec<ResolvedRefsError> {
    let mut errs = Vec::new();
    if let Some(a) = auth {
        // `SecretSlot.secret` is Controller-resolved transport state, never
        // operator input. Clear it before every attempt so a failed lookup or
        // requeue cannot retain an injected or stale credential.
        a.slot_mut().secret = None;
        if let Some(e) = a.resolve(ctx, owner).await {
            errs.push(e);
        }
    }
    if let Some(t) = tls {
        if let Some(e) = t.resolve(ctx, owner).await {
            errs.push(e);
        }
    }
    errs
}

/// Attempt to resolve every Secret reference inside `config` and attach the
/// resolved Secret onto the corresponding `auth.secret` / `tls.certs.secret`
/// field. Returns the collected `ResolvedRefsError`s (empty on full success).
///
/// **Multi-error collection**: auth and tls are resolved independently —
/// a failure on one side does not short-circuit the other. This matches the
/// Gateway-API convention where `ResolvedRefs.message` aggregates all broken
/// refs and `reason` is picked deterministically via
/// `pick_resolved_refs_reason`'s priority table. Without this, an operator
/// who mistypes both Secret names only sees the auth error first, then has
/// to reconcile again to discover the tls error — and the priority table
/// effectively never fires because the slice always has one element.
///
/// Leaving `secret: None` on the unresolved field is safe: gateway providers
/// only build a client when `CredentialState::is_buildable` returns true, so
/// a partially resolved spec never reaches a live client.
/// A webhook shim field that is present in the spec and therefore triggers a
/// deprecation warning. Carries the CRD field name and the migration hint so
/// the WARN message and the unit test assertions share one source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeprecatedShimField {
    /// CRD field name as the operator wrote it (camelCase).
    field: &'static str,
    /// Migration guidance appended to the warning message.
    migrate_to: &'static str,
}

/// Detect which webhook shim fields are present (pure, side-effect free).
///
/// Splitting detection from logging lets unit tests assert exactly which fields
/// would warn without standing up a tracing subscriber. Returns the fields in a
/// stable order (auth, defaultHeaders, requestMethod) so log/test output is
/// deterministic.
fn detect_deprecated_webhook_shim_fields(w: &WebhookServiceConfig) -> Vec<DeprecatedShimField> {
    let mut out = Vec::new();
    if w.auth.is_some() {
        out.push(DeprecatedShimField {
            field: "auth",
            migrate_to: "request.headers.custom with template: \"<prefix>${secretRef:ns/name:key}\"",
        });
    }
    if w.default_headers.is_some() {
        out.push(DeprecatedShimField {
            field: "defaultHeaders",
            migrate_to: "request.headers.custom with source: value",
        });
    }
    if w.request_method.is_some() {
        out.push(DeprecatedShimField {
            field: "requestMethod",
            migrate_to: "request.method.template",
        });
    }
    out
}

/// Emit WARN-level deprecation logs for any webhook shim fields still present.
///
/// The shim fields (`auth`, `defaultHeaders`, `requestMethod`) are accepted for
/// backward compatibility during rolling upgrades but are not used at runtime in
/// this release. They will be removed in the next release (Task D2). This only
/// logs — it never rejects the resource or alters behavior.
fn warn_deprecated_webhook_shim_fields(owner: &ResourceRef, w: &WebhookServiceConfig) {
    for shim in detect_deprecated_webhook_shim_fields(w) {
        tracing::warn!(
            link_sys = %owner.key(),
            deprecated_field = shim.field,
            "LinkSys uses a deprecated webhook configuration field that will be removed in the \
             next release. Migrate to {}.",
            shim.migrate_to,
        );
    }
}

async fn resolve_for_spec(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    config: &mut SystemConfig,
) -> Vec<ResolvedRefsError> {
    match config {
        SystemConfig::Redis(r) => drive_standard(ctx, owner, r.auth.as_mut(), r.tls.as_mut()).await,
        SystemConfig::Etcd(e) => drive_standard(ctx, owner, e.auth.as_mut(), e.tls.as_mut()).await,
        SystemConfig::Elasticsearch(es) => drive_standard(ctx, owner, es.auth.as_mut(), es.tls.as_mut()).await,
        SystemConfig::Webhook(w) => {
            // Emit deprecation warnings for shim fields (auth, defaultHeaders,
            // requestMethod), accepted for backward compat but unused at runtime.
            warn_deprecated_webhook_shim_fields(owner, w);

            let mut errs = Vec::new();

            // Resolve TLS (unchanged).
            if let Some(tls) = w.conn.tls.as_mut() {
                if let Some(e) = tls.resolve(ctx, owner).await {
                    errs.push(e);
                }
            }

            // Resolve every ${secretRef:ns/name:key} referenced in the outbound
            // request templates, carrying the values inline. FULL REBUILD: the
            // map is reconstructed from scratch each reconcile so a deleted
            // Secret drops out (fail-closed via the gateway readiness gate).
            let names: Vec<String> = w.collect_secret_ref_names();
            let (resolved, helper_errs) = resolve_request_secrets(ctx, owner, ResourceKind::LinkSys, &names).await;
            errs.extend(helper_errs);
            w.resolved_secrets = if resolved.is_empty() {
                None
            } else {
                Some(ResolvedSecrets(resolved))
            };

            errs
        }
        SystemConfig::Kafka(kafka_config) => {
            drive_standard(ctx, owner, kafka_config.sasl.as_mut(), kafka_config.tls.as_mut()).await
        }
        SystemConfig::HttpDns(h) => {
            // `connection` is the lean `HttpDnsConnection { tls, timeout_ms }`;
            // its outbound TLS refs get the same treatment as every other
            // provider. Non-TLS secretRef resolution for httpdns (if any is
            // added later) stays deferred; TLS is resolved here like the rest.
            let mut errs = Vec::new();
            if let Some(conn) = h.connection.as_mut() {
                if let Some(tls) = conn.tls.as_mut() {
                    if let Some(e) = tls.resolve(ctx, owner).await {
                        errs.push(e);
                    }
                }
            }
            errs
        }
    }
}

pub(super) async fn resolve_one<R: SecretMaterialResolver>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    secret_ref: &SecretObjectReference,
    resolver: &R,
) -> Result<Option<Secret>, ResolvedRefsError> {
    let outcome = resolve_secret(ctx, owner, "edgion.io", "LinkSys", secret_ref, resolver).await;
    outcome_to_result(outcome).map(Some)
}

#[cfg(test)]
#[path = "tests/link_sys_test.rs"]
mod tests;
