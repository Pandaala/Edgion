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
    resolve_credential, resolve_request_secrets, resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::{
    ControllerCredentialSourceConfig, CredentialSecretInput, DesiredCredentialSource,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, resolved_refs_condition, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::credentials::{ResolveError, SecretMaterialResolver};
use edgion_resources::prelude_resources::LinkSys;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::link_sys::webhook::ResolvedSecrets;
use edgion_resources::resources::link_sys::{
    CredentialSourceConfig, CredentialSourceProvider, LinkSysStatus, SystemConfig,
};

use super::auth_spec::{AuthSpec, OtlpTlsSpec, TlsSpec};
use super::san_guard;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

const OAUTH2_CLIENT_ID_KEY: &str = "clientId";
const OAUTH2_CLIENT_SECRET_KEY: &str = "clientSecret";

struct OAuth2BootstrapResolver;

impl SecretMaterialResolver for OAuth2BootstrapResolver {
    type Material = ();

    fn required_keys(&self) -> &'static [&'static str] {
        &[OAUTH2_CLIENT_ID_KEY, OAUTH2_CLIENT_SECRET_KEY]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let missing = self
            .required_keys()
            .iter()
            .copied()
            .filter(|key| !data.contains_key(*key))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(ResolveError::MissingKeys(missing));
        }
        for key in self.required_keys() {
            let bytes = &data.get(*key).expect("required key checked above").0;
            let value = std::str::from_utf8(bytes).map_err(|error| ResolveError::DecodeError {
                key,
                source: anyhow::anyhow!(error),
            })?;
            if value.is_empty() {
                return Err(ResolveError::DecodeError {
                    key,
                    source: anyhow::anyhow!("value must not be empty"),
                });
            }
        }
        Ok(())
    }
}

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
    let Some(status) = ls.status.get_or_insert_with(LinkSysStatus::default).as_local_mut() else {
        tracing::warn!(
            component = "status",
            kind = "LinkSys",
            "Unprojected status cannot be updated"
        );
        return;
    };
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

fn credential_source_publication_admission_errors(
    source: &CredentialSourceConfig,
    ctx: &HandlerContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    if source.publication.persist && !ctx.runtime().managed_credentials_supported() {
        errors.push("credentialSource.publication.persist=true requires the Kubernetes configuration center".into());
    }
    if !source.publication.persist && !ctx.runtime().single_publisher() {
        errors.push("credentialSource.publication.persist=false is unsupported when ha_mode is all-serve".into());
    }
    errors
}

fn link_sys_admission_errors(ls: &LinkSys, ctx: &HandlerContext) -> Vec<String> {
    let mut errors = ls.validate_config();
    if let SystemConfig::CredentialSource(source) = &ls.spec.config {
        errors.extend(credential_source_publication_admission_errors(source, ctx));
        let CredentialSourceProvider::OAuth2ClientCredentials(provider) = &source.provider;
        if let Some(previous) = &provider.client_authentication.previous_secret_ref {
            let namespace = ls.metadata.namespace.as_deref().unwrap_or("default");
            let active = &provider.client_authentication.active_secret_ref;
            if active.name == previous.name
                && active.effective_namespace(namespace) == previous.effective_namespace(namespace)
            {
                errors.push(
                    "credentialSource activeSecretRef and previousSecretRef resolve to the same Secret".to_string(),
                );
            }
        }
    }
    // Phase 1 guard: reject outbound TLS subjectAltNames/hostname on LinkSys
    // providers where SAN pinning enforcement is not yet implemented.
    errors.extend(san_guard::check_link_sys_san(ls));
    errors
}

#[async_trait::async_trait]
impl ProcessorHandler<LinkSys> for LinkSysHandler {
    fn preparse(&self, ls: &mut LinkSys, ctx: &HandlerContext) -> Vec<String> {
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
        link_sys_admission_errors(ls, ctx)
    }

    async fn parse(&self, mut ls: LinkSys, ctx: &HandlerContext) -> ProcessResult<LinkSys> {
        // Processor lenient admission continues into parse after preparse
        // errors. Recompute the same pure gate before mutating resolved fields
        // so an Accepted=False credential source can never reach the manager.
        let credential_source_admitted = link_sys_admission_errors(&ls, ctx).is_empty();
        let owner = ResourceRef::new(
            ResourceKind::LinkSys,
            ls.metadata.namespace.clone(),
            ls.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old Secret references first (for update scenario) so that a Secret
        // that was previously referenced but is no longer referenced after an update
        // does not trigger spurious requeues.
        ctx.secret_ref_manager().clear_value_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&owner);
        ctx.runtime()
            .credential_source_registry
            .remove_source(&owner.resource_key());

        let resolution = resolve_for_spec(ctx, &owner, &mut ls.spec.config).await;
        let SpecResolution {
            errors,
            credential_inputs,
            manager_config,
        } = resolution;
        if errors.is_empty() {
            if let (Some(config), Some(uid)) = (manager_config, ls.metadata.uid.clone()) {
                if credential_source_admitted {
                    ctx.runtime()
                        .credential_source_registry
                        .replace_source(DesiredCredentialSource {
                            resource: owner.clone(),
                            uid,
                            generation: ls.metadata.generation.unwrap_or_default(),
                            persist: config.publication.persist,
                            memory_max_keys: config.publication.memory_max_keys,
                            config: ControllerCredentialSourceConfig::new(config),
                            secret_inputs: credential_inputs,
                        });
                }
            }
        }
        if ctx
            .runtime()
            .credential_source_registry
            .source(&owner.resource_key())
            .is_none()
        {
            ctx.runtime()
                .credential_source_registry
                .remove_result(&owner.resource_key());
        }
        let generation = ls.metadata.generation;
        apply_resolved_refs_condition(&mut ls, generation, &errors);

        for err in &errors {
            if matches!(err, ResolvedRefsError::InvalidKind { .. }) {
                tracing::warn!(
                    link_sys = %owner.key(),
                    "Credential reference is invalid; update the LinkSys reference"
                );
            } else if needs_operator_secret_edit(err) {
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
        ctx.secret_ref_manager().clear_value_refs(&owner);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&owner);
        ctx.runtime()
            .credential_source_registry
            .forget_source(&owner.resource_key());
        let dependents = ctx.runtime().link_sys_ref_mgr.clear_source_refs(&owner.resource_key());
        for dependent in dependents {
            ctx.requeue(dependent.kind_str(), dependent.resource_key()).await;
        }
        tracing::debug!(
            link_sys = %owner.key(),
            "Cleared SecretRefManager references on LinkSys delete"
        );
    }

    async fn on_change(&self, ls: &LinkSys, ctx: &HandlerContext) {
        let key = ResourceRef::new(
            ResourceKind::LinkSys,
            ls.metadata.namespace.clone(),
            ls.metadata.name.clone().unwrap_or_default(),
        )
        .resource_key();
        for dependent in ctx.runtime().link_sys_ref_mgr.get_refs(&key) {
            ctx.requeue(dependent.kind_str(), dependent.resource_key()).await;
        }
    }

    fn update_status(&self, ls: &mut LinkSys, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ls.metadata.generation;

        // `ResolvedRefs` is maintained by `parse` (see `apply_resolved_refs_condition`);
        // this method owns `Accepted` and the independent unknown-field advisory.
        let Some(status) = ls.status.get_or_insert_with(LinkSysStatus::default).as_local_mut() else {
            tracing::warn!(
                component = "status",
                kind = "LinkSys",
                "Unprojected status cannot be updated"
            );
            return;
        };

        status.observed_generation = None;
        status.credential_state = None;
        status.published_generation = None;
        status.issued_at = None;
        status.valid_until = None;
        status.last_successful_refresh_time = None;
        status.next_refresh_time = None;
        status.desired_consumer_count = None;
        status.published_consumer_count = None;
        status.conflicted_consumer_count = None;

        if matches!(&ls.spec.config, SystemConfig::CredentialSource(_)) {
            let key = format!(
                "{}/{}",
                ls.metadata.namespace.as_deref().unwrap_or("default"),
                ls.metadata.name.as_deref().unwrap_or_default()
            );
            if let Some(result) = ctx
                .runtime()
                .credential_source_registry
                .result(&key)
                .filter(|result| Some(result.observed_generation) == generation)
            {
                status.observed_generation = Some(result.observed_generation);
                status.credential_state = Some(result.state);
                status.published_generation = result.published_generation;
                status.issued_at = result.issued_at;
                status.valid_until = result.valid_until;
                status.last_successful_refresh_time = result.last_successful_refresh_time;
                status.next_refresh_time = result.next_refresh_time;
                status.desired_consumer_count = Some(result.desired_consumer_count);
                status.published_consumer_count = Some(result.published_consumer_count);
                status.conflicted_consumer_count = Some(result.conflicted_consumer_count);
            }
        }

        if validation_errors.is_empty() {
            update_condition(&mut status.conditions, accepted_condition(generation));
        } else {
            let reason = match &ls.spec.config {
                SystemConfig::CredentialSource(source)
                    if source.publication.persist && !ctx.runtime().managed_credentials_supported() =>
                {
                    "UnsupportedConfigurationCenter"
                }
                SystemConfig::CredentialSource(source)
                    if !source.publication.persist && !ctx.runtime().single_publisher() =>
                {
                    "UnsupportedHaMode"
                }
                _ => "Invalid",
            };
            update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    reason,
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/link_sys_test/unknown_fields_tests.rs"]
mod unknown_fields_tests;

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
#[derive(Default)]
struct SpecResolution {
    errors: Vec<ResolvedRefsError>,
    credential_inputs: Vec<CredentialSecretInput>,
    manager_config: Option<edgion_resources::resources::link_sys::CredentialSourceConfig>,
}

fn secret_input(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    role: &'static str,
    secret_ref: &SecretObjectReference,
) -> Option<CredentialSecretInput> {
    let owner_ns = owner.namespace.as_deref().unwrap_or("default");
    let namespace = secret_ref.effective_namespace(owner_ns).to_string();
    let secret = ctx.runtime().secret_store.get(Some(&namespace), &secret_ref.name)?;
    Some(CredentialSecretInput {
        role,
        namespace: namespace.clone(),
        name: secret_ref.name.clone(),
        uid: secret.metadata.uid,
        resource_version: secret.metadata.resource_version,
        authorization_revision: ctx.runtime().ref_grant_store.authorization_revision(
            owner_ns,
            "edgion.io",
            "LinkSys",
            &namespace,
            "",
            "Secret",
            Some(&secret_ref.name),
        ),
    })
}

async fn resolve_credential_source(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    config: &mut edgion_resources::resources::link_sys::CredentialSourceConfig,
) -> SpecResolution {
    let mut result = SpecResolution::default();
    let mut resolved_tls = None;
    let CredentialSourceProvider::OAuth2ClientCredentials(provider) = &mut config.provider;

    // These fields are Controller-resolved transport state on ordinary
    // LinkSys variants. A credential source is Controller-only, so submitted
    // or stale resolved values must never enter conf_sync.
    if let Some(tls) = provider.tls.as_mut() {
        tls.resolved_ca_certificates = None;
        tls.resolved_client_certificate = None;
    }

    let active = &provider.client_authentication.active_secret_ref;
    match resolve_credential(ctx, owner, "edgion.io", "LinkSys", active, &OAuth2BootstrapResolver).await {
        ResolveOutcome::Ok(()) => {
            if let Some(input) = secret_input(ctx, owner, "active-bootstrap", active) {
                result.credential_inputs.push(input);
            }
        }
        outcome => result.errors.push(outcome_to_result(outcome).unwrap_err()),
    }

    if let Some(previous) = &provider.client_authentication.previous_secret_ref {
        match resolve_credential(ctx, owner, "edgion.io", "LinkSys", previous, &OAuth2BootstrapResolver).await {
            ResolveOutcome::Ok(()) => {
                if let Some(input) = secret_input(ctx, owner, "previous-bootstrap", previous) {
                    result.credential_inputs.push(input);
                }
            }
            outcome => result.errors.push(outcome_to_result(outcome).unwrap_err()),
        }
    }

    if let Some(tls) = provider.tls.as_ref().filter(|tls| tls.enabled) {
        let mut resolution_copy = tls.clone();
        if let Some(error) = resolution_copy.resolve(ctx, owner).await {
            result.errors.push(error);
        } else {
            if let Some(refs) = resolution_copy
                .outbound
                .validation
                .as_ref()
                .and_then(|validation| validation.ca_certificate_refs.as_deref())
            {
                for ca_ref in refs.iter().filter(|reference| reference.kind == "Secret") {
                    let reference = SecretObjectReference {
                        group: Some(ca_ref.group.clone()),
                        kind: Some(ca_ref.kind.clone()),
                        name: ca_ref.name.clone(),
                        namespace: ca_ref.namespace.clone(),
                    };
                    if let Some(input) = secret_input(ctx, owner, "tls-ca", &reference) {
                        result.credential_inputs.push(input);
                    }
                }
            }
            if let Some(reference) = resolution_copy.outbound.client_certificate_ref.as_ref() {
                if let Some(input) = secret_input(ctx, owner, "tls-client-certificate", reference) {
                    result.credential_inputs.push(input);
                }
            }
            resolved_tls = Some(resolution_copy);
        }
    }

    let mut manager_config = config.clone();
    let CredentialSourceProvider::OAuth2ClientCredentials(manager_provider) = &mut manager_config.provider;
    manager_provider.tls = resolved_tls;
    result.manager_config = Some(manager_config);

    result
}

async fn resolve_for_spec(ctx: &HandlerContext, owner: &ResourceRef, config: &mut SystemConfig) -> SpecResolution {
    let errors = match config {
        SystemConfig::Redis(r) => drive_standard(ctx, owner, r.auth.as_mut(), r.tls.as_mut()).await,
        SystemConfig::Etcd(e) => drive_standard(ctx, owner, e.auth.as_mut(), e.tls.as_mut()).await,
        SystemConfig::Otlp(o) => {
            let mut tls = o.tls.as_mut().map(OtlpTlsSpec);
            drive_standard(ctx, owner, o.auth.as_mut(), tls.as_mut()).await
        }
        SystemConfig::Elasticsearch(es) => drive_standard(ctx, owner, es.auth.as_mut(), es.tls.as_mut()).await,
        SystemConfig::Webhook(w) => {
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
        SystemConfig::CredentialSource(source) => return resolve_credential_source(ctx, owner, source).await,
    };
    SpecResolution {
        errors,
        credential_inputs: Vec::new(),
        manager_config: None,
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
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/link_sys_test.rs"]
mod tests;
