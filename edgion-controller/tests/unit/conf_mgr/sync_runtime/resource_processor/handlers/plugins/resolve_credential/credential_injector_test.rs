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
    runtime.credential_memory.insert_oauth_publication(
        &source.identity_key(),
        crate::services::credential_manager::memory::GeneratedSecretEntry {
            key: "pub-1".to_string(),
            value: zeroize::Zeroizing::new(token.to_vec()),
            issued_at: "2026-08-27T00:00:00Z".to_string(),
            valid_until: "2099-08-27T12:00:00Z".to_string(),
            valid_until_mono,
        },
    );
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

    let previous_generation = ep.clone();
    insert_memory_token(&runtime, &source, b"rotated-token");
    resolve(&mut ep, &owner, &context).await;
    assert_eq!(resolved_value(&ep), Some("rotated-token"));
    assert_eq!(resolved_value(&previous_generation), Some("access-token"));
    assert_eq!(runtime.credential_memory.key_count(), 1);
    assert!(same_name_secret_deps(&context, &owner).is_empty());
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
