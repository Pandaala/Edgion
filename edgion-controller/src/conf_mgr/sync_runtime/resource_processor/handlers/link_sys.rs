//! LinkSys Handler
//!
//! Handles LinkSys resources with:
//! - Validation (preparse hook)
//! - SecretRef resolution via the bound credential pipeline (attaches Secret to spec)
//! - SecretRefManager registration for cascading updates
//! - Gateway API standard status management
//!
//! Note: the resolved Secret is embedded into the corresponding `*Auth.secret` field,
//! which is serialized across conf_sync to the Gateway. Material parsing happens on
//! the Gateway side via `edgion_resources::credentials`.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_bound_secret, resolve_request_secrets, AuthorizedSecret,
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
use edgion_resources::resources::common::ResolvedSecrets;
use edgion_resources::resources::link_sys::{
    CredentialSourceConfig, CredentialSourceProvider, LinkSysStatus, SystemConfig,
};

use super::auth_spec::{self, AuthSpec, ResolvedTls};
use super::san_guard;
use edgion_resources::resources::refs::link_sys_fields::{
    BoundLinkSysConfigMut, LinkSysReferences, PreparedLinkSysReferences,
};
use edgion_resources::resources::refs::{ReferenceError, ReferenceToken};
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

        let prepared = PreparedLinkSysReferences::from_link_sys(&ls);
        let resolution = match prepared.bind_mut(&mut ls) {
            Ok(mut binding) => resolve_for_spec(ctx, &owner, &mut binding).await,
            Err(error) => SpecResolution {
                errors: vec![ResolvedRefsError::InvalidReference { error }],
                ..Default::default()
            },
        };
        let SpecResolution {
            errors,
            credential_inputs,
            manager_tls,
        } = resolution;
        if errors.is_empty() && credential_source_admitted {
            if let Some(uid) = ls.metadata.uid.clone() {
                if matches!(ls.spec.config, SystemConfig::CredentialSource(_)) {
                    // Clone the scrubbed actual owner before moving in this generation's
                    // authorized material. The returned wire resource stays scrubbed.
                    let mut manager_owner = ls.clone();
                    if let Some(resolved) = manager_tls {
                        let SystemConfig::CredentialSource(config) = &mut manager_owner.spec.config else {
                            unreachable!("credential source variant checked above");
                        };
                        let CredentialSourceProvider::OAuth2ClientCredentials(provider) = &mut config.provider;
                        resolved.apply(provider.tls.as_mut().expect("an enabled TLS loan has an owning block"));
                    }
                    let config = ControllerCredentialSourceConfig::new(manager_owner)
                        .expect("credential source variant checked above");
                    ctx.runtime()
                        .credential_source_registry
                        .replace_source(DesiredCredentialSource {
                            resource: owner.clone(),
                            uid,
                            generation: ls.metadata.generation.unwrap_or_default(),
                            persist: config.publication.persist,
                            config,
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
            if matches!(
                err,
                ResolvedRefsError::InvalidKind { .. } | ResolvedRefsError::InvalidReference { .. }
            ) {
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

#[derive(Default)]
struct SpecResolution {
    errors: Vec<ResolvedRefsError>,
    credential_inputs: Vec<CredentialSecretInput>,
    manager_tls: Option<ResolvedTls>,
}

#[derive(Default)]
struct ProviderResolution {
    auth: Option<Secret>,
    tls: Option<ResolvedTls>,
    templates: Option<ResolvedSecrets>,
    spec: SpecResolution,
}

pub(super) fn secret_input(
    role: &'static str,
    token: ReferenceToken<'_>,
    authorized: &AuthorizedSecret,
) -> Result<CredentialSecretInput, ResolvedRefsError> {
    let reference = token
        .validated()
        .map_err(|error| ResolvedRefsError::InvalidReference { error })?;
    let target = reference.target();
    Ok(CredentialSecretInput {
        reference: token
            .freeze()
            .map_err(|error| ResolvedRefsError::InvalidReference { error })?,
        role,
        namespace: target
            .scope()
            .namespace()
            .ok_or(ResolvedRefsError::InvalidReference {
                error: ReferenceError::MissingNamespace,
            })?
            .to_string(),
        name: target.name().to_string(),
        uid: authorized.secret.metadata.uid.clone(),
        resource_version: authorized.secret.metadata.resource_version.clone(),
        authorization_revision: authorized.authorization_revision.clone(),
    })
}

fn drive_standard<A: AuthSpec>(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    auth: Option<&A>,
    references: &LinkSysReferences<'_>,
) -> ProviderResolution {
    let mut result = ProviderResolution::default();
    if let Some(auth) = auth {
        match references.auth_secret() {
            Some(token) => match auth.resolve(ctx, owner, token) {
                Ok(authorized) => result.auth = Some(authorized.secret),
                Err(error) => result.spec.errors.push(error),
            },
            None => result.spec.errors.push(ResolvedRefsError::InvalidReference {
                error: ReferenceError::StaleBinding,
            }),
        }
    }
    resolve_provider_tls(ctx, owner, references, &mut result);
    result
}

fn resolve_provider_tls(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: &LinkSysReferences<'_>,
    result: &mut ProviderResolution,
) {
    if let Some(references) = references.tls() {
        let tls = auth_spec::resolve_tls(ctx, owner, references);
        if let Some(error) = tls.error.as_ref() {
            result.spec.errors.push(error.clone());
        }
        result.tls = Some(tls);
    }
}

fn clear_materials(config: &mut SystemConfig) {
    fn auth<A: AuthSpec>(value: Option<&mut A>) {
        if let Some(value) = value {
            value.slot_mut().secret = None;
        }
    }
    fn tls(value: Option<&mut edgion_resources::resources::common::TlsConfig>) {
        if let Some(value) = value {
            value.resolved_ca_certificates = None;
            value.resolved_client_certificate = None;
        }
    }
    match config {
        SystemConfig::Redis(value) => {
            auth(value.auth.as_mut());
            tls(value.tls.as_mut());
        }
        SystemConfig::Etcd(value) => {
            auth(value.auth.as_mut());
            tls(value.tls.as_mut());
        }
        SystemConfig::Elasticsearch(value) => {
            auth(value.auth.as_mut());
            tls(value.tls.as_mut());
        }
        SystemConfig::Kafka(value) => {
            auth(value.sasl.as_mut());
            tls(value.tls.as_mut());
        }
        SystemConfig::Otlp(value) => {
            auth(value.auth.as_mut());
            tls(value.tls.as_mut());
        }
        SystemConfig::Webhook(value) => {
            value.resolved_secrets = None;
            tls(value.conn.tls.as_mut());
        }
        SystemConfig::HttpDns(value) => {
            tls(value.connection.as_mut().and_then(|connection| connection.tls.as_mut()));
        }
        SystemConfig::CredentialSource(value) => match &mut value.provider {
            CredentialSourceProvider::OAuth2ClientCredentials(provider) => tls(provider.tls.as_mut()),
        },
        SystemConfig::Unsupported => {}
    }
}

fn apply_materials(config: &mut SystemConfig, mut result: ProviderResolution) -> SpecResolution {
    fn auth<A: AuthSpec>(value: Option<&mut A>, resolved: Option<Secret>) {
        if let Some(value) = value {
            value.slot_mut().secret = resolved;
        }
    }
    fn tls(value: Option<&mut edgion_resources::resources::common::TlsConfig>, resolved: Option<ResolvedTls>) {
        if let (Some(value), Some(resolved)) = (value, resolved) {
            resolved.apply(value);
        }
    }
    match config {
        SystemConfig::Redis(value) => {
            auth(value.auth.as_mut(), result.auth);
            tls(value.tls.as_mut(), result.tls);
        }
        SystemConfig::Etcd(value) => {
            auth(value.auth.as_mut(), result.auth);
            tls(value.tls.as_mut(), result.tls);
        }
        SystemConfig::Elasticsearch(value) => {
            auth(value.auth.as_mut(), result.auth);
            tls(value.tls.as_mut(), result.tls);
        }
        SystemConfig::Kafka(value) => {
            auth(value.sasl.as_mut(), result.auth);
            tls(value.tls.as_mut(), result.tls);
        }
        SystemConfig::Otlp(value) => {
            auth(value.auth.as_mut(), result.auth);
            tls(value.tls.as_mut(), result.tls);
        }
        SystemConfig::Webhook(value) => {
            value.resolved_secrets = result.templates.take();
            tls(value.conn.tls.as_mut(), result.tls);
        }
        SystemConfig::HttpDns(value) => {
            tls(
                value.connection.as_mut().and_then(|connection| connection.tls.as_mut()),
                result.tls,
            );
        }
        // Only the manager-only clone receives resolved TLS for this provider.
        SystemConfig::CredentialSource(_) | SystemConfig::Unsupported => {}
    }
    result.spec
}

fn resolve_credential_source(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    references: &LinkSysReferences<'_>,
) -> ProviderResolution {
    let mut result = ProviderResolution::default();
    for (role, token) in [
        ("active-bootstrap", references.active_bootstrap_secret()),
        ("previous-bootstrap", references.previous_bootstrap_secret()),
    ] {
        let Some(token) = token else {
            if role == "active-bootstrap" {
                result.spec.errors.push(ResolvedRefsError::InvalidReference {
                    error: ReferenceError::StaleBinding,
                });
            }
            continue;
        };
        match resolve_bound_secret(ctx, owner, token, &OAuth2BootstrapResolver) {
            Ok(authorized) => match secret_input(role, token, &authorized) {
                Ok(input) => result.spec.credential_inputs.push(input),
                Err(error) => result.spec.errors.push(error),
            },
            Err(error) => result.spec.errors.push(error),
        }
    }
    resolve_provider_tls(ctx, owner, references, &mut result);
    if let Some(mut resolved) = result.tls.take() {
        if resolved.error.is_none() {
            result.spec.credential_inputs.append(&mut resolved.inputs);
            result.spec.manager_tls = Some(resolved);
        }
    }
    result
}

async fn resolve_for_spec(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    binding: &mut BoundLinkSysConfigMut<'_>,
) -> SpecResolution {
    clear_materials(binding.config_mut());
    let result = {
        let references = match binding.references() {
            Ok(references) => references,
            Err(error) => {
                return SpecResolution {
                    errors: vec![ResolvedRefsError::InvalidReference { error }],
                    ..Default::default()
                }
            }
        };
        match binding.config() {
            SystemConfig::Redis(value) => drive_standard(ctx, owner, value.auth.as_ref(), &references),
            SystemConfig::Etcd(value) => drive_standard(ctx, owner, value.auth.as_ref(), &references),
            SystemConfig::Elasticsearch(value) => drive_standard(ctx, owner, value.auth.as_ref(), &references),
            SystemConfig::Kafka(value) => drive_standard(ctx, owner, value.sasl.as_ref(), &references),
            SystemConfig::Otlp(value) => drive_standard(ctx, owner, value.auth.as_ref(), &references),
            SystemConfig::Webhook(_) => {
                let mut result = ProviderResolution::default();
                resolve_provider_tls(ctx, owner, &references, &mut result);
                let (resolved, errors) = resolve_request_secrets(ctx, owner, references.request_secrets());
                result.spec.errors.extend(errors);
                result.templates = (!resolved.is_empty()).then_some(ResolvedSecrets(resolved));
                result
            }
            SystemConfig::HttpDns(_) => {
                let mut result = ProviderResolution::default();
                resolve_provider_tls(ctx, owner, &references, &mut result);
                result
            }
            SystemConfig::CredentialSource(_) => resolve_credential_source(ctx, owner, &references),
            SystemConfig::Unsupported => ProviderResolution::default(),
        }
    };
    apply_materials(binding.config_mut(), result)
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/link_sys_test.rs"]
mod tests;
