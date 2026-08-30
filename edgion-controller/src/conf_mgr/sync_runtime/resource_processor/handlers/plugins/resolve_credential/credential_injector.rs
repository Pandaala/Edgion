//! CredentialInjector Secret and Controller-managed LinkSys resolution.

use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::resolve_secret_readonly;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, BearerResolver, InvalidCredentialError, ResolveOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::{
    DesiredCredentialConsumer, DesiredCredentialSource, BINDING_REVISION_ANNOTATION, CONSUMER_UID_ANNOTATION,
    MANAGED_BY_LABEL, MANAGED_BY_VALUE, SOURCE_GENERATION_ANNOTATION, SOURCE_UID_ANNOTATION, VALID_UNTIL_ANNOTATION,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::credentials::SecretMaterialResolver;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    ResolvedCredentialInjector, CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN,
};
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, PluginValidation};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

const OWNER_GROUP: &str = "edgion.io";
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
    config: &edgion_resources::resources::edgion_plugins::CredentialInjectorConfig,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
    register_cross_namespace: bool,
) -> Result<DesiredCredentialSource, ResolvedRefsError> {
    let owner_ns = resource_ref.namespace.as_deref().unwrap_or("default");
    let source_ns = config.credential_ref.effective_namespace(owner_ns);
    if source_ns != owner_ns {
        if register_cross_namespace {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(source_ns.to_string(), resource_ref.clone());
        }
        if !ctx.runtime().ref_grant_store.check_reference_allowed(
            owner_ns,
            OWNER_GROUP,
            OWNER_KIND,
            source_ns,
            "edgion.io",
            "LinkSys",
            Some(&config.credential_ref.name),
        ) {
            return Err(ResolvedRefsError::RefNotPermitted {
                from_namespace: owner_ns.to_string(),
                from_kind: OWNER_KIND,
                target_namespace: source_ns.to_string(),
                target_name: config.credential_ref.name.clone(),
            });
        }
    }
    let source_key = format!("{source_ns}/{}", config.credential_ref.name);
    ctx.runtime()
        .credential_source_registry
        .source(&source_key)
        .ok_or_else(|| ResolvedRefsError::LinkSysNotFound {
            namespace: source_ns.to_string(),
            name: config.credential_ref.name.clone(),
        })
}

async fn resolve_link_sys(
    config: &mut edgion_resources::resources::edgion_plugins::CredentialInjectorConfig,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
    consumer_uid: &str,
    consumer_generation: i64,
) -> Result<(), ResolvedRefsError> {
    let owner_ns = resource_ref.namespace.as_deref().unwrap_or("default");
    let source_ns = config.credential_ref.effective_namespace(owner_ns);
    let source_key = format!("{source_ns}/{}", config.credential_ref.name);
    ctx.runtime()
        .link_sys_ref_mgr
        .add_ref(source_key.clone(), resource_ref.clone());

    let source = link_sys_source(config, resource_ref, ctx, true)?;
    if source.persist {
        ctx.secret_ref_manager()
            .add_ref(format!("{owner_ns}/{}", resource_ref.name), resource_ref.clone());
    }
    let consumer = DesiredCredentialConsumer {
        resource: resource_ref.clone(),
        uid: consumer_uid.to_string(),
        generation: consumer_generation,
        source_key,
        source_uid: source.uid.clone(),
        source_generation: source.generation,
        target_secret_namespace: owner_ns.to_string(),
        target_secret_name: resource_ref.name.clone(),
        authorization_revision: ctx.runtime().ref_grant_store.authorization_revision(
            owner_ns,
            OWNER_GROUP,
            OWNER_KIND,
            source_ns,
            "edgion.io",
            "LinkSys",
            Some(&config.credential_ref.name),
        ),
    };
    let binding_revision = consumer.binding_revision();
    ctx.runtime().credential_source_registry.replace_consumer(consumer);

    if !source.persist {
        return resolve_from_memory(config, resource_ref, ctx, &source);
    }

    let output_ref = SecretObjectReference {
        group: None,
        kind: None,
        name: resource_ref.name.clone(),
        namespace: resource_ref.namespace.clone(),
    };
    let result = outcome_to_secret(
        resolve_secret(ctx, resource_ref, OWNER_GROUP, OWNER_KIND, &output_ref, &BearerResolver).await,
    );
    let secret = match result {
        Ok(secret) => secret,
        Err(error) => {
            request_publication_reconcile(ctx, resource_ref);
            return Err(error);
        }
    };
    let valid_until = match validate_managed_secret(
        &secret,
        &resource_ref.name,
        consumer_uid,
        &source.uid,
        source.generation,
        &binding_revision,
    ) {
        Ok(valid_until) => valid_until,
        Err(error) => {
            request_publication_reconcile(ctx, resource_ref);
            return Err(error);
        }
    };
    let data = match secret.data.as_ref() {
        Some(data) => data,
        None => {
            request_publication_reconcile(ctx, resource_ref);
            return Err(ResolvedRefsError::MissingKeys {
                keys: vec!["bearerToken"],
            });
        }
    };
    let material = match BearerResolver.parse(data).map_err(map_resolve_error) {
        Ok(material) => material,
        Err(error) => {
            request_publication_reconcile(ctx, resource_ref);
            return Err(error);
        }
    };
    if !final_value_fits(config.target.value_prefix().len(), material.token.len()) {
        request_publication_reconcile(ctx, resource_ref);
        return Err(managed_secret_error(
            "final injected header value exceeds the configured hard limit",
        ));
    }
    config.resolved_credential = Some(ResolvedCredentialInjector {
        value: material.token,
        valid_until: Some(valid_until.to_string()),
    });
    ctx.runtime()
        .credential_source_registry
        .clear_consumer_reconcile(&resource_ref.resource_key());
    Ok(())
}

fn resolve_from_memory(
    config: &mut edgion_resources::resources::edgion_plugins::CredentialInjectorConfig,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
    source: &DesiredCredentialSource,
) -> Result<(), ResolvedRefsError> {
    match memory_material(ctx, source, config.target.value_prefix().len()) {
        Ok((value, valid_until)) => {
            config.resolved_credential = Some(ResolvedCredentialInjector {
                value,
                valid_until: Some(valid_until),
            });
            ctx.runtime()
                .credential_source_registry
                .clear_consumer_reconcile(&resource_ref.resource_key());
            Ok(())
        }
        Err(error) => {
            request_publication_reconcile(ctx, resource_ref);
            Err(error)
        }
    }
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

fn request_publication_reconcile(ctx: &HandlerContext, resource_ref: &ResourceRef) {
    let namespace = resource_ref.namespace.as_deref().unwrap_or("default");
    let observation = ctx
        .runtime()
        .secret_store
        .get(Some(namespace), &resource_ref.name)
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
        .request_consumer_reconcile(&resource_ref.resource_key(), observation);
}

fn outcome_to_secret(outcome: ResolveOutcome<Secret>) -> Result<Secret, ResolvedRefsError> {
    match outcome {
        ResolveOutcome::Ok(secret) => Ok(secret),
        ResolveOutcome::Pending(
            crate::conf_mgr::sync_runtime::resource_processor::credential::PendingReason::SecretNotFound {
                namespace,
                name,
            },
        ) => Err(ResolvedRefsError::SecretNotFound { namespace, name }),
        ResolveOutcome::Pending(
            crate::conf_mgr::sync_runtime::resource_processor::credential::PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            },
        ) => Err(ResolvedRefsError::RefNotPermitted {
            from_namespace: from_ns,
            from_kind: OWNER_KIND,
            target_namespace: to_ns,
            target_name: secret_name,
        }),
        ResolveOutcome::Invalid(error) => Err(map_invalid_error(error)),
    }
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

/// Clear untrusted transport material and resolve every request-stage injector.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) async fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let consumer_uid = ep.metadata.uid.clone();
    let consumer_generation = ep.metadata.generation.unwrap_or_default();
    // Preserved plugin config objects can carry this recognized but
    // Controller-owned field in any stage. Clear it everywhere before doing
    // request-stage resolution, including entries rejected as wrong-stage.
    for plugin in ep
        .spec
        .request_plugins
        .iter_mut()
        .flatten()
        .map(|entry| &mut entry.plugin)
        .chain(
            ep.spec
                .upstream_response_filter_plugins
                .iter_mut()
                .flatten()
                .map(|entry| &mut entry.plugin),
        )
        .chain(
            ep.spec
                .upstream_response_body_filter_plugins
                .iter_mut()
                .flatten()
                .map(|entry| &mut entry.plugin),
        )
        .chain(
            ep.spec
                .upstream_response_plugins
                .iter_mut()
                .flatten()
                .map(|entry| &mut entry.plugin),
        )
    {
        if let EdgionPlugin::CredentialInjector(config) = plugin {
            config.resolved_credential = None;
        }
    }

    let mut active_link_sys_count = 0usize;
    macro_rules! count_active_link_sys {
        ($entries:expr) => {
            active_link_sys_count += $entries
                .iter()
                .flatten()
                .filter(|entry| entry.is_enabled() && entry.policy_action.is_none())
                .filter(|entry| {
                    matches!(
                        &entry.plugin,
                        EdgionPlugin::CredentialInjector(config) if config.is_link_sys_ref()
                    )
                })
                .count();
        };
    }
    count_active_link_sys!(ep.spec.request_plugins);
    count_active_link_sys!(ep.spec.upstream_response_filter_plugins);
    count_active_link_sys!(ep.spec.upstream_response_body_filter_plugins);
    count_active_link_sys!(ep.spec.upstream_response_plugins);
    if active_link_sys_count > 1 {
        return;
    }

    let Some(entries) = ep.spec.request_plugins.as_mut() else {
        return;
    };
    for entry in entries {
        if !entry.is_enabled() || entry.policy_action.is_some() {
            continue;
        }
        let EdgionPlugin::CredentialInjector(config) = &mut entry.plugin else {
            continue;
        };

        if config.validation_error().is_some() {
            continue;
        }

        if config.is_link_sys_ref() {
            let Some(consumer_uid) = consumer_uid.as_deref() else {
                continue;
            };
            let _ = resolve_link_sys(config, resource_ref, ctx, consumer_uid, consumer_generation).await;
            continue;
        }

        let secret = match resolve_secret(
            ctx,
            resource_ref,
            OWNER_GROUP,
            OWNER_KIND,
            &config.credential_ref,
            &BearerResolver,
        )
        .await
        {
            ResolveOutcome::Ok(secret) if !is_managed_secret(&secret) => secret,
            _ => continue,
        };
        let Some(data) = secret.data.as_ref() else {
            continue;
        };
        let Ok(material) = BearerResolver.parse(data) else {
            continue;
        };
        if !final_value_fits(config.target.value_prefix().len(), material.token.len()) {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_name = %config.credential_ref.name,
                "CredentialInjector: resolved header value exceeds the configured hard limit"
            );
            continue;
        }
        config.resolved_credential = Some(ResolvedCredentialInjector {
            value: material.token,
            valid_until: None,
        });
    }
}

/// Re-run typed content validation without registering dependencies. Pending
/// missing/grant outcomes are omitted because the handler's generic dependency
/// classifier already reports them.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn status_errors(
    ep: &EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    let Some(entries) = ep.spec.request_plugins.as_ref() else {
        return errors;
    };
    for entry in entries {
        let EdgionPlugin::CredentialInjector(config) = &entry.plugin else {
            continue;
        };
        if config.validation_error().is_some() {
            continue;
        }
        if config.is_link_sys_ref() {
            let Some(consumer_uid) = ep.metadata.uid.as_deref() else {
                errors.push(managed_secret_error(
                    "EdgionPlugins UID is unavailable for managed publication",
                ));
                continue;
            };
            let source = match link_sys_source(config, resource_ref, ctx, false) {
                Ok(source) => source,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            if !source.persist {
                if let Err(error) = memory_material(ctx, &source, config.target.value_prefix().len()) {
                    errors.push(error);
                }
                continue;
            }
            let binding_revision = DesiredCredentialConsumer {
                resource: resource_ref.clone(),
                uid: consumer_uid.to_string(),
                generation: ep.metadata.generation.unwrap_or_default(),
                source_key: source.logical_key(),
                source_uid: source.uid.clone(),
                source_generation: source.generation,
                target_secret_namespace: resource_ref.namespace.clone().unwrap_or_else(|| "default".to_string()),
                target_secret_name: resource_ref.name.clone(),
                authorization_revision: ctx.runtime().ref_grant_store.authorization_revision(
                    resource_ref.namespace.as_deref().unwrap_or("default"),
                    OWNER_GROUP,
                    OWNER_KIND,
                    source.resource.namespace.as_deref().unwrap_or("default"),
                    "edgion.io",
                    "LinkSys",
                    Some(&source.resource.name),
                ),
            }
            .binding_revision();
            let output_ref = SecretObjectReference {
                group: None,
                kind: None,
                name: resource_ref.name.clone(),
                namespace: resource_ref.namespace.clone(),
            };
            match resolve_secret_readonly(ctx, resource_ref, OWNER_GROUP, OWNER_KIND, &output_ref, &BearerResolver) {
                ResolveOutcome::Ok(secret) => {
                    if let Err(error) = validate_managed_secret(
                        &secret,
                        &resource_ref.name,
                        consumer_uid,
                        &source.uid,
                        source.generation,
                        &binding_revision,
                    ) {
                        errors.push(error);
                    } else if let Some(data) = secret.data.as_ref() {
                        match BearerResolver.parse(data) {
                            Ok(material)
                                if !final_value_fits(config.target.value_prefix().len(), material.token.len()) =>
                            {
                                errors.push(managed_secret_error(
                                    "final injected header value exceeds the configured hard limit",
                                ))
                            }
                            Ok(_) => {}
                            Err(error) => errors.push(map_resolve_error(error)),
                        }
                    }
                }
                ResolveOutcome::Pending(_) => {}
                ResolveOutcome::Invalid(error) => errors.push(map_invalid_error(error)),
            }
            continue;
        }

        match resolve_secret_readonly(
            ctx,
            resource_ref,
            OWNER_GROUP,
            OWNER_KIND,
            &config.credential_ref,
            &BearerResolver,
        ) {
            ResolveOutcome::Ok(secret) if is_managed_secret(&secret) => errors.push(managed_secret_error(
                "operator-authored Secret references cannot consume a managed OAuth2 output Secret",
            )),
            ResolveOutcome::Ok(secret) => {
                let Some(data) = secret.data.as_ref() else {
                    errors.push(ResolvedRefsError::MissingKeys {
                        keys: vec!["bearerToken"],
                    });
                    continue;
                };
                let material = match BearerResolver.parse(data) {
                    Ok(material) => material,
                    Err(error) => {
                        errors.push(map_resolve_error(error));
                        continue;
                    }
                };
                if !final_value_fits(config.target.value_prefix().len(), material.token.len()) {
                    errors.push(ResolvedRefsError::DecodeError {
                        key: "bearerToken",
                        detail: format!(
                            "final injected header value exceeds the {}-byte limit",
                            CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN
                        ),
                    });
                }
            }
            ResolveOutcome::Pending(_) => {}
            ResolveOutcome::Invalid(error) => errors.push(map_invalid_error(error)),
        }
    }
    errors
}

fn map_invalid_error(error: InvalidCredentialError) -> ResolvedRefsError {
    match error {
        InvalidCredentialError::MissingKeys(keys) => ResolvedRefsError::MissingKeys { keys },
        InvalidCredentialError::DecodeError { key, detail } => ResolvedRefsError::DecodeError { key, detail },
        InvalidCredentialError::InvalidValue { key, class } => ResolvedRefsError::DecodeError {
            key,
            detail: class.to_string(),
        },
        InvalidCredentialError::InvalidSecretType { got, allowed } => {
            ResolvedRefsError::InvalidSecretType { got, allowed }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    use edgion_resources::prelude_resources::EdgionPluginsSpec;
    use edgion_resources::resources::common::SecretObjectReference;
    use edgion_resources::resources::edgion_plugins::{
        CredentialInjectionTarget, CredentialInjectorConfig, RequestFilterEntry, UpstreamResponseFilterEntry,
    };
    use edgion_resources::ResourceKind;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;

    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;

    fn make_context(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let context = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
        (context, runtime)
    }

    fn secret(name: &str, token: &[u8]) -> Secret {
        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            data: Some(BTreeMap::from([(
                "bearerToken".to_string(),
                ByteString(token.to_vec()),
            )])),
            ..Default::default()
        }
    }

    fn managed_secret(name: &str, token: &[u8]) -> Secret {
        let mut secret = secret(name, token);
        secret.metadata.labels = Some(BTreeMap::from([(
            MANAGED_BY_LABEL.to_string(),
            MANAGED_BY_VALUE.to_string(),
        )]));
        secret.metadata.annotations = Some(BTreeMap::from([
            (CONSUMER_UID_ANNOTATION.to_string(), "consumer-uid".to_string()),
            (SOURCE_UID_ANNOTATION.to_string(), "source-uid".to_string()),
            (SOURCE_GENERATION_ANNOTATION.to_string(), "7".to_string()),
            (BINDING_REVISION_ANNOTATION.to_string(), "binding-revision".to_string()),
            (VALID_UNTIL_ANNOTATION.to_string(), "2099-08-27T12:00:00Z".to_string()),
        ]));
        secret.metadata.owner_references = Some(vec![OwnerReference {
            api_version: "edgion.io/v1".to_string(),
            kind: "EdgionPlugins".to_string(),
            name: "injector".to_string(),
            uid: "consumer-uid".to_string(),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }]);
        secret
    }

    fn plugin(resolved_value: Option<&str>) -> EdgionPlugins {
        let mut ep = EdgionPlugins::new("injector", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CredentialInjector(
            CredentialInjectorConfig {
                credential_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "provider-token".to_string(),
                    namespace: None,
                },
                target: CredentialInjectionTarget::Bearer,
                resolved_credential: resolved_value.map(|value| ResolvedCredentialInjector {
                    value: value.to_string(),
                    valid_until: None,
                }),
            },
        ))]);
        ep
    }

    fn resolved_value(ep: &EdgionPlugins) -> Option<&str> {
        let entry = ep.spec.request_plugins.as_ref()?.first()?;
        let EdgionPlugin::CredentialInjector(config) = &entry.plugin else {
            return None;
        };
        config
            .resolved_credential
            .as_ref()
            .map(|resolved| resolved.value.as_str())
    }

    #[test]
    fn complete_value_limit_is_inclusive_and_overflow_safe() {
        assert!(final_value_fits(7, CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN - 7));
        assert!(!final_value_fits(7, CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN - 6));
        assert!(!final_value_fits(usize::MAX, 1));
    }

    #[test]
    fn invalid_bearer_syntax_maps_without_material() {
        let error = map_invalid_error(InvalidCredentialError::InvalidValue {
            key: "bearerToken",
            class: edgion_resources::credentials::InvalidValueClass::BearerToken,
        });
        assert!(matches!(
            error,
            ResolvedRefsError::DecodeError { key: "bearerToken", .. }
        ));
        assert!(!error.message().contains("credential-value"));
    }

    #[test]
    fn managed_secret_requires_exact_owner_and_binding_identity() {
        let secret = managed_secret("injector", b"token");
        assert_eq!(
            validate_managed_secret(&secret, "injector", "consumer-uid", "source-uid", 7, "binding-revision",).unwrap(),
            "2099-08-27T12:00:00Z"
        );
        assert!(validate_managed_secret(
            &secret,
            "injector",
            "recreated-uid",
            "source-uid",
            7,
            "binding-revision",
        )
        .is_err());
        assert!(validate_managed_secret(
            &secret,
            "injector",
            "consumer-uid",
            "other-source",
            7,
            "binding-revision",
        )
        .is_err());
        assert!(
            validate_managed_secret(&secret, "injector", "consumer-uid", "source-uid", 8, "binding-revision",).is_err()
        );
        assert!(validate_managed_secret(&secret, "injector", "consumer-uid", "source-uid", 7, "old-binding",).is_err());
    }

    #[test]
    fn ordinary_secret_reference_rejects_managed_output_marker() {
        let (context, _runtime) = make_context(HashMap::from([(
            "default/provider-token".to_string(),
            managed_secret("provider-token", b"token"),
        )]));
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let ep = plugin(None);
        let errors = status_errors(&ep, &owner, &context);
        assert!(errors.iter().any(|error| error
            .message()
            .contains("operator-authored Secret references cannot consume")));
    }

    #[tokio::test]
    async fn resolution_replaces_forged_material_and_registers_dependency() {
        let (context, _runtime) = make_context(HashMap::from([(
            "default/provider-token".to_string(),
            secret("provider-token", b"real-token"),
        )]));
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = plugin(Some("forged-token"));

        resolve(&mut ep, &owner, &context).await;

        assert_eq!(resolved_value(&ep), Some("real-token"));
        assert_eq!(
            context.secret_ref_manager().get_dependencies(&owner.key()),
            vec!["default/provider-token"]
        );
    }

    #[tokio::test]
    async fn missing_secret_clears_material_and_readonly_status_avoids_duplicate() {
        let (context, _runtime) = make_context(HashMap::new());
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = plugin(Some("forged-token"));

        resolve(&mut ep, &owner, &context).await;

        assert_eq!(resolved_value(&ep), None);
        assert_eq!(
            context.secret_ref_manager().get_dependencies(&owner.key()),
            vec!["default/provider-token"]
        );
        assert!(status_errors(&ep, &owner, &context).is_empty());
    }

    #[tokio::test]
    async fn forged_material_is_cleared_even_in_a_wrong_stage_entry() {
        let (context, _runtime) = make_context(HashMap::new());
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = plugin(None);
        let mut forged_config = match &ep.spec.request_plugins.as_ref().unwrap()[0].plugin {
            EdgionPlugin::CredentialInjector(config) => config.clone(),
            _ => panic!("unexpected plugin type"),
        };
        forged_config.resolved_credential = Some(ResolvedCredentialInjector {
            value: "forged-token".to_string(),
            valid_until: None,
        });
        ep.spec.upstream_response_filter_plugins = Some(vec![UpstreamResponseFilterEntry::new(
            EdgionPlugin::CredentialInjector(forged_config),
        )]);

        resolve(&mut ep, &owner, &context).await;

        let entry = &ep.spec.upstream_response_filter_plugins.as_ref().unwrap()[0];
        let EdgionPlugin::CredentialInjector(config) = &entry.plugin else {
            panic!("unexpected plugin type");
        };
        assert!(config.resolved_credential.is_none());
    }

    #[test]
    fn readonly_status_does_not_register_or_duplicate_grant_denial() {
        let (context, _runtime) = make_context(HashMap::new());
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = plugin(None);
        let Some(entry) = ep.spec.request_plugins.as_mut().and_then(|entries| entries.first_mut()) else {
            panic!("missing CredentialInjector entry");
        };
        let EdgionPlugin::CredentialInjector(config) = &mut entry.plugin else {
            panic!("unexpected plugin type");
        };
        config.credential_ref.namespace = Some("shared".to_string());

        assert!(status_errors(&ep, &owner, &context).is_empty());
        assert!(context.secret_ref_manager().get_dependencies(&owner.key()).is_empty());
    }

    #[tokio::test]
    async fn malformed_and_oversized_material_remain_unresolved_and_surface_status() {
        let tokens = [
            b"bad token".to_vec(),
            vec![b'a'; CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN],
        ];
        for token in &tokens {
            let (context, _runtime) = make_context(HashMap::from([(
                "default/provider-token".to_string(),
                secret("provider-token", token),
            )]));
            let owner = ResourceRef::new(
                ResourceKind::EdgionPlugins,
                Some("default".to_string()),
                "injector".to_string(),
            );
            let mut ep = plugin(None);

            resolve(&mut ep, &owner, &context).await;

            assert_eq!(resolved_value(&ep), None);
            let errors = status_errors(&ep, &owner, &context);
            assert_eq!(errors.len(), 1, "got {errors:?}");
            assert!(matches!(
                errors[0],
                ResolvedRefsError::DecodeError { key: "bearerToken", .. }
            ));
        }
    }

    fn credential_source_config() -> edgion_resources::resources::link_sys::CredentialSourceConfig {
        serde_yaml::from_str(
            r#"
provider:
  type: oauth2ClientCredentials
  tokenEndpoint: https://auth.example.com/oauth2/token
  clientAuthentication:
    activeSecretRef:
      name: oauth-client
"#,
        )
        .expect("valid credentialSource config")
    }

    fn link_sys_desired_source(persist: bool) -> DesiredCredentialSource {
        DesiredCredentialSource {
            resource: ResourceRef::new(
                ResourceKind::LinkSys,
                Some("default".to_string()),
                "source".to_string(),
            ),
            uid: "source-uid".to_string(),
            generation: 7,
            persist,
            memory_max_keys: 10_000,
            config: crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::ControllerCredentialSourceConfig::new(
                credential_source_config(),
            ),
            secret_inputs: Vec::new(),
        }
    }

    fn link_sys_plugin() -> EdgionPlugins {
        let mut ep = EdgionPlugins::new("injector", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".to_string());
        ep.metadata.uid = Some("consumer-uid".to_string());
        ep.metadata.generation = Some(3);
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CredentialInjector(
            CredentialInjectorConfig {
                credential_ref: SecretObjectReference {
                    group: Some("edgion.io".to_string()),
                    kind: Some("LinkSys".to_string()),
                    name: "source".to_string(),
                    namespace: None,
                },
                target: CredentialInjectionTarget::Bearer,
                resolved_credential: None,
            },
        ))]);
        ep
    }

    fn insert_memory_token_until(
        runtime: &ControllerRuntime,
        source: &DesiredCredentialSource,
        token: &[u8],
        valid_until_mono: std::time::Instant,
    ) {
        runtime
            .credential_memory
            .insert_oauth_publication(
                &source.identity_key(),
                crate::services::credential_manager::memory::GeneratedSecretEntry {
                    key: "pub-1".to_string(),
                    value: zeroize::Zeroizing::new(token.to_vec()),
                    issued_at: "2026-08-27T00:00:00Z".to_string(),
                    valid_until: "2099-08-27T12:00:00Z".to_string(),
                    valid_until_mono,
                },
                source.memory_max_keys,
            )
            .expect("memory insert");
    }

    fn insert_memory_token(runtime: &ControllerRuntime, source: &DesiredCredentialSource, token: &[u8]) {
        insert_memory_token_until(
            runtime,
            source,
            token,
            std::time::Instant::now() + std::time::Duration::from_secs(3_600),
        );
    }

    fn same_name_secret_deps(context: &HandlerContext, owner: &ResourceRef) -> Vec<String> {
        context
            .secret_ref_manager()
            .get_dependencies(&owner.key())
            .into_iter()
            .filter(|key| key == &format!("{}/{}", owner.namespace.as_deref().unwrap_or("default"), owner.name))
            .collect()
    }

    fn resolved_until(ep: &EdgionPlugins) -> Option<&str> {
        let entry = ep.spec.request_plugins.as_ref()?.first()?;
        let EdgionPlugin::CredentialInjector(config) = &entry.plugin else {
            return None;
        };
        config
            .resolved_credential
            .as_ref()
            .and_then(|resolved| resolved.valid_until.as_deref())
    }

    #[tokio::test]
    async fn persist_off_memory_hit_embeds_token_without_same_name_secret_dep() {
        let (context, runtime) = make_context(HashMap::new());
        let source = link_sys_desired_source(false);
        runtime.credential_source_registry.replace_source(source.clone());
        insert_memory_token(&runtime, &source, b"access-token");
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = link_sys_plugin();

        resolve(&mut ep, &owner, &context).await;

        assert_eq!(resolved_value(&ep), Some("access-token"));
        assert_eq!(resolved_until(&ep), Some("2099-08-27T12:00:00Z"));
        assert!(
            same_name_secret_deps(&context, &owner).is_empty(),
            "persist-off must not register {{owner_ns}}/{{plugin_name}} as a generated-secret dep"
        );
        assert!(
            runtime
                .credential_source_registry
                .consumer(&owner.resource_key())
                .is_some(),
            "memory-mode resolve must still register DesiredCredentialConsumer"
        );
        assert!(
            !runtime.link_sys_ref_mgr.get_refs("default/source").is_empty(),
            "memory-mode resolve must still register link_sys_ref_mgr"
        );
    }

    #[tokio::test]
    async fn persist_off_memory_miss_leaves_unresolved_but_registers_consumer() {
        let (context, runtime) = make_context(HashMap::new());
        runtime
            .credential_source_registry
            .replace_source(link_sys_desired_source(false));
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = link_sys_plugin();

        resolve(&mut ep, &owner, &context).await;

        assert_eq!(resolved_value(&ep), None);
        assert!(
            runtime
                .credential_source_registry
                .consumer(&owner.resource_key())
                .is_some(),
            "a memory miss must still register DesiredCredentialConsumer"
        );
        assert!(
            same_name_secret_deps(&context, &owner).is_empty(),
            "persist-off miss must not register the same-name Secret dependency"
        );
    }

    #[tokio::test]
    async fn persist_off_expired_memory_stays_unresolved_and_requests_reconcile() {
        let (context, runtime) = make_context(HashMap::new());
        let source = link_sys_desired_source(false);
        runtime.credential_source_registry.replace_source(source.clone());
        insert_memory_token_until(
            &runtime,
            &source,
            b"expired-token",
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = link_sys_plugin();

        resolve(&mut ep, &owner, &context).await;

        assert_eq!(resolved_value(&ep), None);
        assert!(
            !status_errors(&ep, &owner, &context).is_empty(),
            "an expired memory credential must surface as unresolved"
        );
        assert!(
            runtime
                .credential_source_registry
                .reconcile_epoch_for_source(&source.logical_key())
                > 0,
            "an expired memory credential must request reacquisition"
        );
    }

    #[tokio::test]
    async fn persist_on_link_sys_still_requires_managed_secret() {
        let source = link_sys_desired_source(true);
        let (context, runtime) = make_context(HashMap::new());
        runtime.credential_source_registry.replace_source(source.clone());
        insert_memory_token(&runtime, &source, b"memory-only-token");
        let owner = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("default".to_string()),
            "injector".to_string(),
        );
        let mut ep = link_sys_plugin();

        resolve(&mut ep, &owner, &context).await;

        assert_eq!(
            resolved_value(&ep),
            None,
            "persist-on must not embed from controller memory"
        );
        assert_eq!(
            same_name_secret_deps(&context, &owner),
            vec!["default/injector"],
            "persist-on must still depend on the managed same-name Secret"
        );

        let consumer = DesiredCredentialConsumer {
            resource: owner.clone(),
            uid: "consumer-uid".to_string(),
            generation: 3,
            source_key: "default/source".to_string(),
            source_uid: "source-uid".to_string(),
            source_generation: 7,
            target_secret_namespace: "default".to_string(),
            target_secret_name: "injector".to_string(),
            authorization_revision: "same-namespace".to_string(),
        };
        let mut secret = managed_secret("injector", b"secret-token");
        secret
            .metadata
            .annotations
            .as_mut()
            .expect("annotations")
            .insert(BINDING_REVISION_ANNOTATION.to_string(), consumer.binding_revision());
        runtime
            .secret_store
            .replace_all(HashMap::from([("default/injector".to_string(), secret)]));
        let mut ep = link_sys_plugin();
        resolve(&mut ep, &owner, &context).await;
        assert_eq!(resolved_value(&ep), Some("secret-token"));
        assert_eq!(resolved_until(&ep), Some("2099-08-27T12:00:00Z"));
    }
}
