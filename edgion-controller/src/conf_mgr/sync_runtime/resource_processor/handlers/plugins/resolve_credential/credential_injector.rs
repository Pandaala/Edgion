//! CredentialInjector Secret and Controller-managed LinkSys resolution.

use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::{
    resolve_bound_secret_readonly, resolve_managed_credential_output,
};
use crate::conf_mgr::sync_runtime::resource_processor::credential::{resolve_bound_secret, BearerResolver};
use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::{
    DesiredCredentialConsumer, DesiredCredentialSource, BINDING_REVISION_ANNOTATION, CONSUMER_UID_ANNOTATION,
    MANAGED_BY_LABEL, MANAGED_BY_VALUE, SOURCE_GENERATION_ANNOTATION, SOURCE_UID_ANNOTATION, VALID_UNTIL_ANNOTATION,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    ResolvedCredentialInjector, CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN,
};
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, PluginValidation};
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};
use edgion_resources::resources::refs::{ReferenceError, ReferenceToken};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

const OWNER_KIND: &str = "EdgionPlugins";

fn final_value_fits(prefix_len: usize, credential_len: usize) -> bool {
    prefix_len
        .checked_add(credential_len)
        .is_some_and(|len| len <= CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN)
}

fn is_managed_secret(secret: &Secret) -> bool {
    secret
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(MANAGED_BY_LABEL))
        .is_some_and(|value| value == MANAGED_BY_VALUE)
}

fn managed_secret_error(detail: &'static str) -> ResolvedRefsError {
    ResolvedRefsError::DecodeError {
        key: "bearerToken",
        detail: detail.to_string(),
    }
}

fn validate_managed_secret<'a>(
    secret: &'a Secret,
    consumer_name: &str,
    consumer_uid: &str,
    source_uid: &str,
    source_generation: i64,
    binding_revision: &str,
) -> Result<&'a str, ResolvedRefsError> {
    if !is_managed_secret(secret) {
        return Err(managed_secret_error(
            "same-name Secret is not managed by the OAuth2 credential controller",
        ));
    }
    let owned = secret
        .metadata
        .owner_references
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|owner| {
            owner.api_version == "edgion.io/v1"
                && owner.kind == "EdgionPlugins"
                && owner.name == consumer_name
                && owner.uid == consumer_uid
                && owner.controller == Some(true)
        });
    if !owned {
        return Err(managed_secret_error(
            "same-name Secret is not owned by the current EdgionPlugins UID",
        ));
    }
    let annotations = secret
        .metadata
        .annotations
        .as_ref()
        .ok_or_else(|| managed_secret_error("managed Secret is missing credential publication annotations"))?;
    if annotations.get(CONSUMER_UID_ANNOTATION).map(String::as_str) != Some(consumer_uid)
        || annotations.get(SOURCE_UID_ANNOTATION).map(String::as_str) != Some(source_uid)
        || annotations.get(SOURCE_GENERATION_ANNOTATION).map(String::as_str)
            != Some(source_generation.to_string().as_str())
        || annotations.get(BINDING_REVISION_ANNOTATION).map(String::as_str) != Some(binding_revision)
    {
        return Err(managed_secret_error(
            "managed Secret publication identity does not match the current binding",
        ));
    }
    let valid_until = annotations
        .get(VALID_UNTIL_ANNOTATION)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| managed_secret_error("managed Secret is missing a validity deadline"))?;
    if edgion_common::time::parse_rfc3339_deadline(valid_until, SystemTime::now())
        .is_none_or(|remaining| remaining.is_zero())
    {
        return Err(managed_secret_error(
            "managed Secret credential is expired or has an invalid deadline",
        ));
    }
    Ok(valid_until)
}

fn link_sys_source(
    token: ReferenceToken<'_>,
    owner: &ResourceRef,
    ctx: &HandlerContext,
    register: bool,
) -> Result<(DesiredCredentialSource, String), ResolvedRefsError> {
    let reference = token
        .validated()
        .map_err(|error| ResolvedRefsError::InvalidReference { error })?;
    let identity = reference.owner();
    if identity.name() != owner.name
        || identity.group_kind().kind() != owner.kind_str()
        || identity.scope().namespace() != Some(owner.namespace.as_deref().unwrap_or("default"))
    {
        return Err(ResolvedRefsError::InvalidReference {
            error: ReferenceError::StaleBinding,
        });
    }
    let target = reference.target();
    if target.group_kind().group() != "edgion.io" || target.group_kind().kind() != "LinkSys" {
        return Err(ResolvedRefsError::InvalidReference {
            error: ReferenceError::UnsupportedTarget,
        });
    }
    let namespace = target.scope().namespace().ok_or(ResolvedRefsError::InvalidReference {
        error: ReferenceError::MissingNamespace,
    })?;
    let key = format!("{namespace}/{}", target.name());
    if register {
        ctx.runtime().link_sys_ref_mgr.add_ref(key.clone(), owner.clone());
        if !reference.is_same_namespace() {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(namespace.to_string(), owner.clone());
        }
    }
    let revision = ctx
        .runtime()
        .ref_grant_store
        .query_authorization_revision(reference.grant_query())
        .ok_or_else(|| ResolvedRefsError::RefNotPermitted {
            from_namespace: owner.namespace.clone().unwrap_or_else(|| "default".to_string()),
            from_kind: OWNER_KIND,
            target_namespace: namespace.to_string(),
            target_name: target.name().to_string(),
        })?;
    let source =
        ctx.runtime()
            .credential_source_registry
            .source(&key)
            .ok_or_else(|| ResolvedRefsError::LinkSysNotFound {
                namespace: namespace.to_string(),
                name: target.name().to_string(),
            })?;
    Ok((source, revision))
}

fn consumer_binding(
    token: ReferenceToken<'_>,
    owner: &ResourceRef,
    uid: &str,
    generation: i64,
    source: &DesiredCredentialSource,
    authorization_revision: String,
) -> Result<DesiredCredentialConsumer, ResolvedRefsError> {
    Ok(DesiredCredentialConsumer {
        reference: token
            .freeze()
            .map_err(|error| ResolvedRefsError::InvalidReference { error })?,
        resource: owner.clone(),
        uid: uid.to_string(),
        generation,
        source_key: source.logical_key(),
        source_uid: source.uid.clone(),
        source_generation: source.generation,
        target_secret_namespace: owner.namespace.clone().unwrap_or_else(|| "default".to_string()),
        target_secret_name: owner.name.clone(),
        authorization_revision,
    })
}

fn link_sys_material(
    config: &edgion_resources::resources::edgion_plugins::CredentialInjectorConfig,
    token: ReferenceToken<'_>,
    owner: &ResourceRef,
    ctx: &HandlerContext,
    uid: &str,
    generation: i64,
    register: bool,
) -> Result<ResolvedCredentialInjector, ResolvedRefsError> {
    let (source, revision) = link_sys_source(token, owner, ctx, register)?;
    let consumer = consumer_binding(token, owner, uid, generation, &source, revision)?;
    if register {
        ctx.runtime()
            .credential_source_registry
            .replace_consumer(consumer.clone());
    }
    let result = if !source.persist {
        memory_material(ctx, &source, config.target.value_prefix().len()).map(|(value, deadline)| {
            ResolvedCredentialInjector {
                value,
                valid_until: Some(deadline),
            }
        })
    } else {
        (|| {
            let authorized = resolve_managed_credential_output(ctx, owner, token, register)?;
            let valid_until = validate_managed_secret(
                &authorized.secret,
                &owner.name,
                uid,
                &source.uid,
                source.generation,
                &consumer.binding_revision(),
            )?;
            let data = authorized.secret.data.as_ref().ok_or(ResolvedRefsError::MissingKeys {
                keys: vec!["bearerToken"],
            })?;
            let material = BearerResolver.parse(data).map_err(map_resolve_error)?;
            if !final_value_fits(config.target.value_prefix().len(), material.token.len()) {
                return Err(managed_secret_error(
                    "final injected header value exceeds the configured hard limit",
                ));
            }
            Ok(ResolvedCredentialInjector {
                value: material.token,
                valid_until: Some(valid_until.to_string()),
            })
        })()
    };
    if register {
        if result.is_ok() {
            ctx.runtime()
                .credential_source_registry
                .clear_consumer_reconcile(&owner.resource_key());
        } else {
            request_publication_reconcile(ctx, &consumer);
        }
    }
    result
}

fn memory_material(
    ctx: &HandlerContext,
    source: &DesiredCredentialSource,
    prefix_len: usize,
) -> Result<(String, String), ResolvedRefsError> {
    let Some(entry) = ctx.runtime().credential_memory.active(&source.identity_key()) else {
        return Err(ResolvedRefsError::MissingKeys {
            keys: vec!["bearerToken"],
        });
    };
    if entry.valid_until_mono <= Instant::now() {
        return Err(managed_secret_error("memory credential is expired"));
    }
    let data = BTreeMap::from([("bearerToken".to_string(), ByteString(entry.value.to_vec()))]);
    let material = BearerResolver.parse(&data).map_err(map_resolve_error)?;
    if !final_value_fits(prefix_len, material.token.len()) {
        return Err(managed_secret_error(
            "final injected header value exceeds the configured hard limit",
        ));
    }
    Ok((material.token, entry.valid_until.clone()))
}

fn request_publication_reconcile(ctx: &HandlerContext, consumer: &DesiredCredentialConsumer) {
    let namespace = consumer.resource.namespace.as_deref().unwrap_or("default");
    let observation = ctx
        .runtime()
        .secret_store
        .get(Some(namespace), &consumer.resource.name)
        .map(|secret| {
            format!(
                "{}:{}",
                secret.metadata.uid.as_deref().unwrap_or_default(),
                secret.metadata.resource_version.as_deref().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "missing".to_string());
    ctx.runtime()
        .credential_source_registry
        .request_consumer_reconcile_if_current(consumer, observation);
}

fn map_resolve_error(error: edgion_resources::credentials::ResolveError) -> ResolvedRefsError {
    match error {
        edgion_resources::credentials::ResolveError::MissingKeys(keys) => ResolvedRefsError::MissingKeys { keys },
        edgion_resources::credentials::ResolveError::DecodeError { key, source } => ResolvedRefsError::DecodeError {
            key,
            detail: source.to_string(),
        },
        edgion_resources::credentials::ResolveError::InvalidValue { key, class } => ResolvedRefsError::DecodeError {
            key,
            detail: class.to_string(),
        },
        edgion_resources::credentials::ResolveError::InvalidSecretType { got, allowed } => {
            ResolvedRefsError::InvalidSecretType { got, allowed }
        }
    }
}

/// Clear every stage before resolving supported active owning fields.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) async fn resolve(
    ep: &mut EdgionPlugins,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) {
    let prepared = PreparedPluginReferences::from_plugins(ep);
    let uid = ep.metadata.uid.clone();
    let generation = ep.metadata.generation.unwrap_or_default();
    let count = ep
        .spec
        .request_plugins
        .iter()
        .flatten()
        .map(|e| (e.is_enabled(), &e.plugin))
        .chain(
            ep.spec
                .upstream_response_filter_plugins
                .iter()
                .flatten()
                .map(|e| (e.is_enabled(), &e.plugin)),
        )
        .chain(
            ep.spec
                .upstream_response_body_filter_plugins
                .iter()
                .flatten()
                .map(|e| (e.is_enabled(), &e.plugin)),
        )
        .chain(
            ep.spec
                .upstream_response_plugins
                .iter()
                .flatten()
                .map(|e| (e.is_enabled(), &e.plugin)),
        )
        .filter(|(enabled, plugin)| {
            *enabled && matches!(plugin, EdgionPlugin::CredentialInjector(config) if config.is_link_sys_ref())
        })
        .count();
    let Ok(mut bindings) = prepared.bind_mut(ep) else {
        return;
    };
    for entry in bindings.request_entries() {
        resolve_entry(entry, owner, ctx, uid.as_deref(), generation, count <= 1);
    }
    for entry in bindings.response_filter_entries() {
        clear_entry(entry);
    }
    for entry in bindings.response_body_filter_entries() {
        clear_entry(entry);
    }
    for entry in bindings.response_entries() {
        clear_entry(entry);
    }
}

fn clear_entry(mut entry: BoundPluginEntryMut<'_>) {
    if let EdgionPlugin::CredentialInjector(config) = entry.config_mut() {
        config.resolved_credential = None;
    }
}

fn injector_material(
    config: &edgion_resources::resources::edgion_plugins::CredentialInjectorConfig,
    token: ReferenceToken<'_>,
    owner: &ResourceRef,
    ctx: &HandlerContext,
    uid: Option<&str>,
    generation: i64,
    register: bool,
) -> Result<ResolvedCredentialInjector, ResolvedRefsError> {
    let reference = token
        .validated()
        .map_err(|error| ResolvedRefsError::InvalidReference { error })?;
    if config.validation_error().is_some() {
        return Err(managed_secret_error("CredentialInjector configuration is invalid"));
    }
    if reference.target().group_kind().kind() == "LinkSys" {
        let uid =
            uid.ok_or_else(|| managed_secret_error("EdgionPlugins UID is unavailable for managed publication"))?;
        return link_sys_material(config, token, owner, ctx, uid, generation, register);
    }
    let authorized = if register {
        resolve_bound_secret(ctx, owner, token, &BearerResolver)
    } else {
        resolve_bound_secret_readonly(ctx, owner, token, &BearerResolver)
    }?;
    if is_managed_secret(&authorized.secret) {
        return Err(managed_secret_error(
            "operator-authored Secret references cannot consume a managed OAuth2 output Secret",
        ));
    }
    let data = authorized.secret.data.as_ref().ok_or(ResolvedRefsError::MissingKeys {
        keys: vec!["bearerToken"],
    })?;
    let material = BearerResolver.parse(data).map_err(map_resolve_error)?;
    if !final_value_fits(config.target.value_prefix().len(), material.token.len()) {
        return Err(managed_secret_error(
            "final injected header value exceeds the configured hard limit",
        ));
    }
    Ok(ResolvedCredentialInjector {
        value: material.token,
        valid_until: None,
    })
}

fn resolve_entry(
    mut entry: BoundPluginEntryMut<'_>,
    owner: &ResourceRef,
    ctx: &HandlerContext,
    uid: Option<&str>,
    generation: i64,
    active_count_valid: bool,
) {
    let EdgionPlugin::CredentialInjector(config) = entry.config_mut() else {
        return;
    };
    config.resolved_credential = None;
    if !entry.is_enabled() || !active_count_valid {
        return;
    }
    let material = {
        let EdgionPlugin::CredentialInjector(config) = entry.config() else {
            return;
        };
        let Ok(references) = entry.references() else {
            return;
        };
        let Some(token) = references.credential() else {
            return;
        };
        injector_material(config, token, owner, ctx, uid, generation, true).ok()
    };
    if let EdgionPlugin::CredentialInjector(config) = entry.config_mut() {
        config.resolved_credential = material;
    }
}

/// Derive status from the same actual owning fields without adding edges.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn status_errors(
    ep: &EdgionPlugins,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) -> Vec<ResolvedRefsError> {
    let prepared = PreparedPluginReferences::from_plugins(ep);
    let Ok(bindings) = prepared.bind(ep) else {
        return vec![ResolvedRefsError::InvalidReference {
            error: ReferenceError::StaleBinding,
        }];
    };
    let mut errors = Vec::new();
    for entry in bindings.request_entries() {
        if !entry.is_enabled() {
            continue;
        }
        let EdgionPlugin::CredentialInjector(config) = entry.config() else {
            continue;
        };
        let Ok(references) = entry.references() else {
            errors.push(ResolvedRefsError::InvalidReference {
                error: ReferenceError::StaleBinding,
            });
            continue;
        };
        let Some(token) = references.credential() else {
            continue;
        };
        let is_link_sys = token
            .validated()
            .is_ok_and(|reference| reference.target().group_kind().kind() == "LinkSys");
        if let Err(error) = injector_material(
            config,
            token,
            owner,
            ctx,
            ep.metadata.uid.as_deref(),
            ep.metadata.generation.unwrap_or_default(),
            false,
        ) {
            // Valid pending targets are reported by the dependency classifier.
            // Invalid identities have no dependency edge and must remain visible.
            if !matches!(error, ResolvedRefsError::SecretNotFound { .. })
                && (!matches!(error, ResolvedRefsError::RefNotPermitted { .. }) || is_link_sys)
            {
                errors.push(error);
            }
        }
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/credential_injector_test.rs"]
mod tests;
