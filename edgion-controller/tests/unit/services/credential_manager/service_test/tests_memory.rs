use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime};

use edgion_resources::credentials::resolvers::bearer::BEARER_TOKEN_KEY;
use edgion_resources::resources::link_sys::CredentialSourceConfig;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::ControllerCredentialSourceConfig;
use crate::conf_mgr::sync_runtime::resource_processor::ResourceRef;
use crate::conf_mgr::ControllerRuntime;

use super::*;

fn credential_source_config() -> CredentialSourceConfig {
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

fn persist_source(persist: bool) -> DesiredCredentialSource {
    DesiredCredentialSource {
        resource: ResourceRef::new(ResourceKind::LinkSys, Some("ops".to_string()), "source".to_string()),
        uid: "source-uid".to_string(),
        generation: 2,
        persist,
        memory_max_keys: 10_000,
        config: ControllerCredentialSourceConfig::new(credential_source_config()),
        secret_inputs: Vec::new(),
    }
}

fn memory_consumer(name: &str) -> DesiredCredentialConsumer {
    DesiredCredentialConsumer {
        resource: ResourceRef::new(ResourceKind::EdgionPlugins, Some("apps".to_string()), name.to_string()),
        uid: format!("uid-{name}"),
        generation: 1,
        source_key: "ops/source".to_string(),
        source_uid: "source-uid".to_string(),
        source_generation: 2,
        target_secret_namespace: "apps".to_string(),
        target_secret_name: name.to_string(),
        authorization_revision: "grant-rv".to_string(),
    }
}

fn retained_publication() -> RetainedPublication {
    let issued_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    RetainedPublication {
        access_token: "access-token".to_string(),
        publication_generation: "pub-1".to_string(),
        issued_at,
        valid_until: issued_at + Duration::from_secs(3_600),
        expires_at: Instant::now() + Duration::from_secs(3_600),
        next_refresh_at: Instant::now() + Duration::from_secs(900),
    }
}

fn recoverable_secret(
    source: &DesiredCredentialSource,
    consumer: &DesiredCredentialConsumer,
    input_revision: &str,
) -> Secret {
    let identity = ManagedSecretIdentity {
        consumer: ConsumerIdentity {
            namespace: consumer.target_secret_namespace.clone(),
            name: consumer.target_secret_name.clone(),
            uid: consumer.uid.clone(),
            generation: consumer.generation,
        },
        source: SourceIdentity {
            name: source.logical_key(),
            uid: source.uid.clone(),
            generation: source.generation,
        },
    };
    let issued_at = SystemTime::now();
    let valid_until = issued_at + Duration::from_secs(3_600);
    Secret {
        metadata: ObjectMeta {
            name: Some(consumer.target_secret_name.clone()),
            namespace: Some(consumer.target_secret_namespace.clone()),
            labels: Some(BTreeMap::from([(
                secret_io::LABEL_MANAGED_BY.to_string(),
                secret_io::MANAGED_BY_VALUE.to_string(),
            )])),
            annotations: Some(BTreeMap::from([
                (
                    secret_io::ANNOTATION_CONSUMER_NAME.to_string(),
                    consumer.target_secret_name.clone(),
                ),
                (secret_io::ANNOTATION_CONSUMER_UID.to_string(), consumer.uid.clone()),
                (secret_io::ANNOTATION_SOURCE_NAME.to_string(), source.logical_key()),
                (secret_io::ANNOTATION_SOURCE_UID.to_string(), source.uid.clone()),
                (
                    secret_io::ANNOTATION_PUBLICATION_GENERATION.to_string(),
                    "recovered-pub".to_string(),
                ),
                (
                    secret_io::ANNOTATION_SOURCE_INPUT_REVISION.to_string(),
                    input_revision.to_string(),
                ),
                (
                    secret_io::ANNOTATION_BINDING_REVISION.to_string(),
                    consumer.binding_revision(),
                ),
                (secret_io::ANNOTATION_ISSUED_AT.to_string(), format_rfc3339(issued_at)),
                (
                    secret_io::ANNOTATION_VALID_UNTIL.to_string(),
                    format_rfc3339(valid_until),
                ),
                (secret_io::ANNOTATION_LEADER_TERM.to_string(), "leader-1".to_string()),
            ])),
            uid: Some("secret-uid".to_string()),
            owner_references: Some(vec![secret_io::owner_reference(&identity)]),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            BEARER_TOKEN_KEY.to_string(),
            ByteString(b"access-token".to_vec()),
        )])),
        ..Default::default()
    }
}

#[test]
fn publish_to_memory_stores_token_without_touching_secret_store() {
    let runtime = ControllerRuntime::new();
    let (bootstrap_key, bootstrap) = (
        "ops/oauth-client".to_string(),
        Secret {
            metadata: ObjectMeta {
                name: Some("oauth-client".to_string()),
                namespace: Some("ops".to_string()),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    runtime
        .secret_store
        .replace_all(HashMap::from([(bootstrap_key, bootstrap)]));
    let secret_count_before = runtime.secret_store.count();
    let bootstrap_before = runtime.secret_store.get(Some("ops"), "oauth-client");

    let source = persist_source(false);
    let retained = retained_publication();
    publish_to_memory(&runtime, &source, &retained).expect("persist-off publish");

    assert_eq!(runtime.secret_store.count(), secret_count_before);
    assert_eq!(
        runtime.secret_store.get(Some("ops"), "oauth-client"),
        bootstrap_before,
        "persist-off publish must not mutate the Secret store"
    );
    let active = runtime
        .credential_memory
        .active(&source.identity_key())
        .expect("memory slot must hold the active token");
    assert_eq!(active.key, retained.publication_generation);
    assert_eq!(&*active.value, retained.access_token.as_bytes());
    assert_eq!(active.issued_at, format_rfc3339(retained.issued_at));
    assert_eq!(active.valid_until, format_rfc3339(retained.valid_until));
}

#[test]
fn publish_to_memory_skips_insert_when_persist_is_on() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(true);
    publish_to_memory(&runtime, &source, &retained_publication()).expect("persist-on skip");
    assert!(
        runtime.credential_memory.active(&source.identity_key()).is_none(),
        "persist-on must not insert into credential memory"
    );
}

#[test]
fn publish_to_memory_skips_insert_when_not_single_publisher() {
    let runtime = ControllerRuntime::new();
    runtime.set_single_publisher(false);
    let source = persist_source(false);
    publish_to_memory(&runtime, &source, &retained_publication()).expect("all-serve skip");
    assert!(
        runtime.credential_memory.active(&source.identity_key()).is_none(),
        "persist-off under all-serve must not insert into credential memory"
    );
}

#[test]
fn publish_to_memory_skips_insert_when_live_source_flipped_to_persist_on() {
    let runtime = ControllerRuntime::new();
    let snapshot = persist_source(false);
    runtime.credential_source_registry.replace_source(persist_source(true));
    publish_to_memory(&runtime, &snapshot, &retained_publication()).expect("live persist-on skip");
    assert!(
        runtime.credential_memory.active(&snapshot.identity_key()).is_none(),
        "a live persist-on source must not receive a memory insert"
    );
}

#[test]
fn recover_source_state_does_not_retain_when_persist_is_off() {
    let runtime = ControllerRuntime::new();
    let persist_on = persist_source(true);
    let consumer = memory_consumer("plugin");
    let input_revision = "input-rev";
    let secret = recoverable_secret(&persist_on, &consumer, input_revision);
    runtime.secret_store.replace_all(HashMap::from([(
        format!("{}/{}", consumer.target_secret_namespace, consumer.target_secret_name),
        secret,
    )]));

    let recovered_on = recover_source_state(&runtime, &persist_on, std::slice::from_ref(&consumer), input_revision);
    assert!(
        recovered_on.retained.is_some(),
        "fixture must recover a persist-on publication from the leftover Secret"
    );

    let persist_off = persist_source(false);
    let recovered_off = recover_source_state(&runtime, &persist_off, &[consumer], input_revision);
    assert!(
        recovered_off.retained.is_none(),
        "persist-off must always reacquire and must not adopt leftover Secrets"
    );
}

#[tokio::test]
async fn publish_manager_result_counts_every_consumer_when_persist_off() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(false);
    runtime.credential_source_registry.replace_source(source.clone());
    runtime
        .credential_source_registry
        .replace_consumer(memory_consumer("a"));
    runtime
        .credential_source_registry
        .replace_consumer(memory_consumer("b"));

    let retained = retained_publication();
    let state = SourceState {
        identity: source.identity_key(),
        retained: Some(retained.clone()),
        last_publication_generation: Some(retained.publication_generation.clone()),
        last_issued_at: Some(retained.issued_at),
        last_valid_until: Some(retained.valid_until),
        ..SourceState::default()
    };
    publish_manager_result(&runtime, &source, &state).await;
    let result = runtime
        .credential_source_registry
        .result(&source.logical_key())
        .expect("manager result");
    assert_eq!(result.desired_consumer_count, 2);
    assert_eq!(
        result.published_consumer_count, 2,
        "persist-off with a retained token counts every current consumer as published"
    );
}

#[tokio::test]
async fn expired_persist_off_state_clears_memory_reports_expired_and_notifies_consumers() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(false);
    let consumer = memory_consumer("injector");
    runtime.credential_source_registry.replace_source(source.clone());
    runtime.credential_source_registry.replace_consumer(consumer.clone());
    let retained = retained_publication();
    publish_to_memory(&runtime, &source, &retained).expect("seed memory");
    let mut state = SourceState {
        identity: source.identity_key(),
        expires_at: Some(Instant::now() - Duration::from_secs(1)),
        retained: Some(retained.clone()),
        last_publication_generation: Some(retained.publication_generation.clone()),
        last_issued_at: Some(retained.issued_at),
        // A wall-clock rollback must not override the monotonic expiry decision.
        last_valid_until: Some(SystemTime::now() + Duration::from_secs(3_600)),
        ..SourceState::default()
    };

    assert!(clear_expired_credential(&runtime, &source, &mut state));
    publish_manager_result(&runtime, &source, &state).await;
    notify_consumers(&runtime, std::slice::from_ref(&consumer), "expired:pub-1".to_string()).await;

    assert!(runtime.credential_memory.active(&source.identity_key()).is_none());
    assert!(state.retained.is_none());
    assert!(state.expires_at.is_none());
    assert!(state.expired);
    let result = runtime
        .credential_source_registry
        .result(&source.logical_key())
        .expect("manager result");
    assert_eq!(result.state, CredentialSourceState::Expired);
    assert_eq!(result.published_consumer_count, 0);
    assert!(
        runtime
            .credential_source_registry
            .reconcile_epoch_for_source(&source.logical_key())
            > 0
    );
}

#[test]
fn last_consumer_clear_drops_the_memory_slot() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(false);
    publish_to_memory(&runtime, &source, &retained_publication()).expect("seed memory");
    assert!(runtime.credential_memory.active(&source.identity_key()).is_some());
    clear_source_memory(&runtime, &source);
    assert!(
        runtime.credential_memory.active(&source.identity_key()).is_none(),
        "removing the last consumer must drop the per-LinkSys memory slot"
    );
}

#[test]
fn unregister_drops_memory_without_consuming_leftover_cleanup_state() {
    for with_leftover_publication in [false, true] {
        let runtime = ControllerRuntime::new();
        let source = persist_source(false);
        publish_to_memory(&runtime, &source, &retained_publication()).expect("seed memory");
        let publications = if with_leftover_publication {
            HashMap::from([("apps/plugin".to_string(), leftover_delete_expectation())])
        } else {
            Default::default()
        };
        let state = SourceState {
            identity: source.identity_key(),
            publications,
            ..SourceState::default()
        };

        forget_unregistered_source_memory(&runtime, &state);

        assert!(
            runtime.credential_memory.active(&source.identity_key()).is_none(),
            "source delete/type change/admission drop must drop the persist-off memory slot"
        );
        assert_eq!(
            state.publications.len(),
            usize::from(with_leftover_publication),
            "memory teardown must leave fenced Secret cleanup expectations available for retry"
        );
    }
}

#[test]
fn identity_replacement_drops_previous_uid_slot() {
    let runtime = ControllerRuntime::new();
    let previous = persist_source(false);
    publish_to_memory(&runtime, &previous, &retained_publication()).expect("seed previous uid");
    let mut next = persist_source(false);
    next.uid = "source-uid-replaced".to_string();
    forget_replaced_identity_memory(&runtime, &previous.identity_key(), &next.identity_key());
    assert!(
        runtime.credential_memory.active(&previous.identity_key()).is_none(),
        "a new source uid must drop the previous identity slot before the next publication"
    );
    assert!(
        runtime.credential_memory.active(&next.identity_key()).is_none(),
        "identity replacement must not insert into the new slot by itself"
    );
}

#[test]
fn persist_off_to_on_keeps_memory_until_published_equals_desired() {
    let runtime = ControllerRuntime::new();
    let persist_off = persist_source(false);
    publish_to_memory(&runtime, &persist_off, &retained_publication()).expect("seed memory");
    let persist_on = persist_source(true);
    runtime.credential_source_registry.replace_source(persist_on.clone());

    maybe_clear_memory_after_fan_out(&runtime, &persist_on, 0, 1);
    assert!(
        runtime.credential_memory.active(&persist_off.identity_key()).is_some(),
        "false->true must keep the memory slot until every consumer Secret is published"
    );

    maybe_clear_memory_after_fan_out(&runtime, &persist_on, 1, 1);
    assert!(
        runtime.credential_memory.active(&persist_off.identity_key()).is_none(),
        "false->true must clear_source once published_consumer_count == desired_consumer_count"
    );
}

#[test]
fn persist_off_does_not_clear_memory_when_counts_already_match() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(false);
    publish_to_memory(&runtime, &source, &retained_publication()).expect("seed memory");
    maybe_clear_memory_after_fan_out(&runtime, &source, 2, 2);
    assert!(
        runtime.credential_memory.active(&source.identity_key()).is_some(),
        "persist-off must not clear_source just because published == desired"
    );
}

#[test]
fn persist_on_to_off_reuses_retained_token_for_memory_insert() {
    let previous = persist_source(true);
    let next = persist_source(false);
    let retained = retained_publication();
    let reused = retained_for_attempt(
        previous.persist,
        next.persist,
        true,
        false,
        false,
        Some(retained.clone()),
    );
    assert!(
        reused.is_some(),
        "true->false must copy the current retained token instead of forcing a reacquire"
    );
    assert_eq!(reused.as_ref().unwrap().access_token, retained.access_token);

    let runtime = ControllerRuntime::new();
    publish_to_memory(&runtime, &next, reused.as_ref().unwrap()).expect("insert retained into memory");
    let active = runtime
        .credential_memory
        .active(&next.identity_key())
        .expect("memory slot after true->false");
    assert_eq!(&*active.value, retained.access_token.as_bytes());
}

#[test]
fn persist_unchanged_does_not_reuse_retained_when_input_revision_changes() {
    let source = persist_source(true);
    let retained = retained_publication();
    assert!(
        retained_for_attempt(source.persist, source.persist, true, false, true, Some(retained)).is_none(),
        "an ordinary input-revision change must still reacquire"
    );
}

fn leftover_delete_expectation() -> DeleteExpectation {
    DeleteExpectation {
        identity: ManagedSecretIdentity {
            consumer: ConsumerIdentity {
                namespace: "apps".to_string(),
                name: "plugin".to_string(),
                uid: "uid-plugin".to_string(),
                generation: 1,
            },
            source: SourceIdentity {
                name: "ops/source".to_string(),
                uid: "source-uid".to_string(),
                generation: 2,
            },
        },
        binding_revision: "binding-rev".to_string(),
        secret_uid: "secret-uid".to_string(),
        publication_generation: "pub-old".to_string(),
        source_input_revision: "input-rev".to_string(),
        leader_term: "leader-1".to_string(),
    }
}

#[test]
fn persist_off_success_keeps_leftover_delete_failures() {
    let leftover = leftover_delete_expectation();
    let mut publications = HashMap::new();
    publications.insert("apps/plugin".to_string(), leftover.clone());
    let success = reconcile_ok(Duration::from_secs(1), retained_publication(), publications);
    assert_eq!(
        success.publications.get("apps/plugin"),
        Some(&leftover),
        "persist-off success must return leftover Secret delete failures so the next tick retries them"
    );
    assert!(
        !success.publications.is_empty(),
        "HashMap::new() must not overwrite leftover-delete failures on persist-off"
    );
}

#[test]
fn memory_only_backend_acquires_persist_off_only() {
    assert!(
        CredentialManagerBackend::MemoryOnly.may_acquire(&persist_source(false)),
        "kube-less worker must acquire persist-off sources"
    );
    assert!(
        !CredentialManagerBackend::MemoryOnly.may_acquire(&persist_source(true)),
        "kube-less worker must not acquire persist-on sources"
    );
}

#[test]
fn kube_less_persist_off_publish_inserts_without_client() {
    let _guard = lock_test_service();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    rt.block_on(async {
        stop_credential_manager_service();
        let generation = install_test_generation();
        let runtime = ControllerRuntime::new();
        let source = persist_source(false);
        runtime.credential_source_registry.replace_source(source.clone());
        assert!(
            try_publish_persist_off(&runtime, &source, &retained_publication(), generation).is_ok(),
            "persist-off publish must succeed without a Kubernetes Client"
        );
        assert!(
            runtime.credential_memory.active(&source.identity_key()).is_some(),
            "kube-less persist-off publish must insert the token into controller memory"
        );
        stop_credential_manager_service();
    });
}

#[test]
fn persist_off_publish_rejects_stale_generation() {
    let _guard = lock_test_service();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    rt.block_on(async {
        stop_credential_manager_service();
        let stale = install_test_generation();
        stop_credential_manager_service();
        let runtime = ControllerRuntime::new();
        let source = persist_source(false);
        runtime.credential_source_registry.replace_source(source.clone());
        assert!(
            !is_current_generation(stale),
            "stopped worker generation must not remain current"
        );
        let result = try_publish_persist_off(&runtime, &source, &retained_publication(), stale);
        assert!(
            result.is_err(),
            "persist-off publish must reject a stale worker generation before the memory write"
        );
        assert!(
            runtime.credential_memory.active(&source.identity_key()).is_none(),
            "a stale generation must not insert into credential memory"
        );
    });
}

#[test]
fn reconciled_continue_on_missing_source_clears_memory_slot() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(false);
    publish_to_memory(&runtime, &source, &retained_publication()).expect("seed memory");
    assert!(runtime.credential_memory.active(&source.identity_key()).is_some());
    let current = take_current_for_reconciled(
        &runtime,
        &source.logical_key(),
        &source.identity_key(),
        &source_input_revision(&source),
    );
    assert!(current.is_none(), "missing source must ignore the Reconciled job");
    assert!(
        runtime.credential_memory.active(&source.identity_key()).is_none(),
        "first persist-off publish must not leak the slot when the source disappears before identity is applied"
    );
}

#[test]
fn reconciled_continue_on_identity_mismatch_clears_job_identity() {
    let runtime = ControllerRuntime::new();
    let source = persist_source(false);
    publish_to_memory(&runtime, &source, &retained_publication()).expect("seed memory");
    let mut current = source.clone();
    current.uid = "source-uid-replaced".to_string();
    runtime.credential_source_registry.replace_source(current);
    let applied = take_current_for_reconciled(
        &runtime,
        &source.logical_key(),
        &source.identity_key(),
        &source_input_revision(&source),
    );
    assert!(applied.is_none(), "uid change must ignore the in-flight Reconciled job");
    assert!(
        runtime.credential_memory.active(&source.identity_key()).is_none(),
        "identity mismatch continue path must clear the in-flight identity slot"
    );
}
