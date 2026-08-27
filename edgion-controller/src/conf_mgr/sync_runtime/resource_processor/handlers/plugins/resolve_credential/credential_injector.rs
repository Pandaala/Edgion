//! CredentialInjector Secret resolution.

use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::resolve_credential_readonly;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_credential, BearerResolver, InvalidCredentialError, ResolveOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    ResolvedCredentialInjector, CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN,
};
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, PluginValidation};

const OWNER_GROUP: &str = "edgion.io";
const OWNER_KIND: &str = "EdgionPlugins";

fn final_value_fits(prefix_len: usize, credential_len: usize) -> bool {
    prefix_len
        .checked_add(credential_len)
        .is_some_and(|len| len <= CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN)
}

/// Clear untrusted transport material and resolve every request-stage injector.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) async fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
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

    let Some(entries) = ep.spec.request_plugins.as_mut() else {
        return;
    };
    for entry in entries {
        let EdgionPlugin::CredentialInjector(config) = &mut entry.plugin else {
            continue;
        };

        if config.validation_error().is_some() {
            continue;
        }

        let outcome = resolve_credential(
            ctx,
            resource_ref,
            OWNER_GROUP,
            OWNER_KIND,
            &config.credential_ref,
            &BearerResolver,
        )
        .await;
        let ResolveOutcome::Ok(material) = outcome else {
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
        config.resolved_credential = Some(ResolvedCredentialInjector { value: material.token });
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
        match resolve_credential_readonly(
            ctx,
            resource_ref,
            OWNER_GROUP,
            OWNER_KIND,
            &config.credential_ref,
            &BearerResolver,
        ) {
            ResolveOutcome::Ok(material) => {
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
}
