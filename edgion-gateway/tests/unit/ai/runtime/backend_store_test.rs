use super::*;
use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
use edgion_resources::resources::edgion_backend::{
    AiBackendDefaults, AiBackendSpec, AiCredential, AiCredentialPool, AiModel, AiModelPricing, EdgionBackendSpec,
};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn resource(name: &str, endpoint: &str, credential: Option<&[u8]>) -> Arc<EdgionBackend> {
    let secret = credential.map(|value| Secret {
        data: Some(BTreeMap::from([("api-key".to_string(), ByteString(value.to_vec()))])),
        ..Default::default()
    });
    let spec = EdgionBackendSpec {
        current_status: None,
        ai: AiBackendSpec {
            provider: AiProvider::OpenAICompatible,
            endpoint: Some(endpoint.to_string()),
            allow_insecure_http: false,
            credential_pool: AiCredentialPool {
                redis_ref: None,
                on_redis_failure: OnRedisFailure::Deny,
                credentials: vec![AiCredential {
                    name: "primary".to_string(),
                    weight: 1,
                    credential: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "provider-key".to_string(),
                            namespace: None,
                        },
                        secret,
                    },
                    limits: None,
                }],
            },
            models: vec![AiModel {
                name: "model-a".to_string(),
                aliases: vec!["fast".to_string()],
                public: true,
                pricing: None,
            }],
            defaults: AiBackendDefaults {
                streaming: true,
                max_output_tokens: Some(2048),
            },
        },
        reconcile_outcome: None,
    };
    let mut resource = EdgionBackend::new(name, spec);
    resource.metadata.namespace = Some("default".to_string());
    Arc::new(resource)
}

#[test]
fn starts_empty() {
    let store = AiBackendStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());
    assert!(store.get_by_key("default/missing").is_none());
}

#[test]
fn empty_noop_update_preserves_snapshot_identity_and_generation() {
    let store = EdgionBackendStore::new();
    let initial = store.snapshot();

    store.partial_update(HashMap::new(), HashMap::new(), HashSet::new());
    let empty_noop = store.snapshot();
    assert_eq!(empty_noop.generation(), 0);
    assert!(Arc::ptr_eq(&initial.inner, &empty_noop.inner));
}

#[test]
fn full_set_atomically_publishes_compiled_service() {
    let store = AiBackendStore::new();
    let resources = HashMap::from([(
        "default/provider".to_string(),
        resource(
            "provider",
            "https://ai.example.com:8443/openai/v1/",
            Some(b"secret-value"),
        ),
    )]);
    store.full_set(&resources);

    assert_eq!(store.count(), 1);
    assert_eq!(store.keys(), vec!["default/provider"]);
    let backend = store.get_by_key("default/provider").expect("compiled backend");
    assert_eq!(backend.uid, "edgion.io/EdgionBackend/default/provider");
    assert_eq!(backend.endpoint.domain, "ai.example.com");
    assert_eq!(backend.endpoint.port, 8443);
    assert!(backend.endpoint.use_tls);
    assert_eq!(backend.endpoint.sni, "ai.example.com");
    assert_eq!(backend.endpoint.host, "ai.example.com:8443");
    assert_eq!(backend.endpoint.path_prefix, "/openai/v1");
    assert!(backend.streaming);
    assert_eq!(backend.max_output_tokens, Some(2048));
    assert_eq!(backend.credential_pool.credentials.len(), 1);
    assert_eq!(backend.credential_pool.credentials[0].name, "primary");
    assert_eq!(backend.credential_pool.credentials[0].credential, "secret-value");
    assert_eq!(backend.models["model-a"].canonical_name, "model-a");
    assert_eq!(backend.models["fast"].canonical_name, "model-a");
}

#[test]
fn runtime_catalog_resolves_longest_wildcard_and_visibility() {
    let mut configured = resource("provider", "https://ai.example.com", Some(b"secret-value"));
    let ai = &mut Arc::get_mut(&mut configured).unwrap().spec.ai;
    ai.models = vec![
        AiModel {
            name: "broad".to_string(),
            aliases: vec!["gpt*".to_string()],
            public: false,
            pricing: None,
        },
        AiModel {
            name: "specific".to_string(),
            aliases: vec!["gpt-4*".to_string()],
            public: true,
            pricing: None,
        },
    ];
    let store = AiBackendStore::new();
    store.replace_all(&HashMap::from([("default/provider".to_string(), configured)]));
    let backend = store.get_by_key("default/provider").unwrap();
    assert_eq!(backend.resolve_model("gpt-4.1").unwrap().canonical_name, "specific");
    assert_eq!(backend.resolve_model("gpt-3").unwrap().canonical_name, "broad");
    assert!(backend.public_model_ids.contains("gpt-4*"));
    assert!(!backend.public_model_ids.contains("gpt*"));
}

#[test]
fn model_pricing_is_compiled_once_and_shared_by_aliases() {
    let mut configured = resource("provider", "https://ai.example.com", Some(b"secret-value"));
    Arc::get_mut(&mut configured).unwrap().spec.ai.models[0].pricing = Some(AiModelPricing {
        input_per_million_usd: "15".to_string(),
        output_per_million_usd: "75".to_string(),
        cache_read_input_per_million_usd: Some("1.5".to_string()),
        cache_write_input_per_million_usd: None,
    });
    let backend = try_compile_backend("default/provider", &configured).unwrap();
    let canonical = backend.models["model-a"].pricing.unwrap();
    let alias = backend.models["fast"].pricing.unwrap();
    assert_eq!(canonical, alias);
    assert_eq!(canonical.input.get(), 15_000_000);
    assert_eq!(canonical.cache_read_input.get(), 1_500_000);
    assert_eq!(canonical.cache_write_input, canonical.input);
}

#[test]
fn partial_update_adds_updates_removes_and_deactivates_invalid_generation() {
    let store = AiBackendStore::new();
    store.partial_update(
        HashMap::from([(
            "default/a".to_string(),
            resource("a", "https://a.example.com", Some(b"a-key")),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    assert_eq!(
        store.get_by_key("default/a").unwrap().credential_pool.credentials[0].credential,
        "a-key"
    );

    store.partial_update(
        HashMap::from([(
            "default/b".to_string(),
            resource("b", "https://b.example.com", Some(b"b-key")),
        )]),
        HashMap::from([(
            "default/a".to_string(),
            resource("a", "https://updated.example.com/prefix", Some(b"new-key")),
        )]),
        HashSet::new(),
    );
    assert_eq!(store.count(), 2);
    assert_eq!(
        store.get_by_key("default/a").unwrap().credential_pool.credentials[0].credential,
        "new-key"
    );

    store.partial_update(
        HashMap::new(),
        HashMap::from([(
            "default/a".to_string(),
            resource("a", "https://updated.example.com", None),
        )]),
        HashSet::from(["default/b".to_string()]),
    );
    assert_eq!(store.count(), 0);
}

#[test]
fn full_set_omits_unresolved_and_invalid_resources() {
    let store = AiBackendStore::new();
    let resources = HashMap::from([
        (
            "default/unresolved".to_string(),
            resource("unresolved", "https://ai.example.com", None),
        ),
        (
            "default/invalid".to_string(),
            resource("invalid", "http://ai.example.com", Some(b"key")),
        ),
    ]);
    store.full_set(&resources);
    assert_eq!(store.count(), 0);
}

#[test]
fn compile_backend_keeps_partial_good_credential_pool() {
    let mut configured = resource("provider", "https://ai.example.com", Some(b"usable-key"));
    let spec = &mut Arc::get_mut(&mut configured).unwrap().spec.ai;
    let mut unresolved = spec.credential_pool.credentials[0].clone();
    unresolved.name = "secondary".to_string();
    unresolved.credential.secret_ref.name = "missing-key".to_string();
    unresolved.credential.secret = None;
    spec.credential_pool.credentials.push(unresolved);

    let backend =
        try_compile_backend("default/provider", &configured).expect("one valid credential keeps backend active");
    assert_eq!(backend.credential_pool.credentials.len(), 1);
    assert_eq!(backend.credential_pool.credentials[0].name, "primary");
    assert_eq!(backend.credential_pool.credentials[0].credential, "usable-key");
}

#[test]
fn compile_backend_preserves_credential_pool_policy_and_limits() {
    let mut configured = resource("provider", "https://ai.example.com", Some(b"usable-key"));
    let credential_pool = &mut Arc::get_mut(&mut configured).unwrap().spec.ai.credential_pool;
    credential_pool.redis_ref = Some("default/ai-limits".to_string());
    credential_pool.on_redis_failure = OnRedisFailure::Allow;
    credential_pool.credentials[0].weight = 17;
    credential_pool.credentials[0].limits = Some(AiCredentialLimits {
        requests_per_minute: Some(120),
        tokens_per_minute: Some(50_000),
    });

    let backend = try_compile_backend("default/provider", &configured).expect("valid limited credential pool");
    assert_eq!(backend.credential_pool.redis_ref.as_deref(), Some("default/ai-limits"));
    assert_eq!(backend.credential_pool.on_redis_failure, OnRedisFailure::Allow);
    let credential = &backend.credential_pool.credentials[0];
    assert_eq!(credential.weight, 17);
    assert_eq!(credential.limits.as_ref().unwrap().requests_per_minute, Some(120));
    assert_eq!(credential.limits.as_ref().unwrap().tokens_per_minute, Some(50_000));
}

#[test]
fn snapshot_keeps_one_service_generation() {
    let store = AiBackendStore::new();
    store.partial_update(
        HashMap::from([(
            "default/provider".to_string(),
            resource("provider", "https://old.example.com", Some(b"old-key")),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    let snapshot = store.snapshot();

    store.partial_update(
        HashMap::new(),
        HashMap::from([(
            "default/provider".to_string(),
            resource("provider", "https://new.example.com", Some(b"new-key")),
        )]),
        HashSet::new(),
    );

    assert_eq!(
        snapshot.get_by_key("default/provider").unwrap().endpoint.domain,
        "old.example.com"
    );
    assert_eq!(
        store.get_by_key("default/provider").unwrap().endpoint.domain,
        "new.example.com"
    );
}

#[test]
fn credential_limiter_survives_unrelated_service_updates() {
    let store = AiBackendStore::new();
    let original = resource("provider", "https://ai.example.com", Some(b"key"));
    store.full_set(&HashMap::from([("default/provider".to_string(), original)]));
    let before = store.get_by_key("default/provider").unwrap();

    let mut updated = resource("provider", "https://ai.example.com", Some(b"key"));
    Arc::get_mut(&mut updated).unwrap().spec.ai.models[0]
        .aliases
        .push("balanced".to_string());
    store.partial_update(
        HashMap::new(),
        HashMap::from([("default/provider".to_string(), updated)]),
        HashSet::new(),
    );

    let after = store.get_by_key("default/provider").unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(Arc::ptr_eq(
        &before.credential_pool.limiter_core,
        &after.credential_pool.limiter_core
    ));
}

#[test]
fn snapshot_multi_get_preserves_order_missing_slots_and_generation() {
    let store = AiBackendStore::new();
    store.partial_update(
        HashMap::from([
            (
                "default/a".to_string(),
                resource("a", "https://a-old.example.com", Some(b"a-key")),
            ),
            (
                "default/b".to_string(),
                resource("b", "https://b-old.example.com", Some(b"b-key")),
            ),
        ]),
        HashMap::new(),
        HashSet::new(),
    );
    let snapshot = store.snapshot();

    store.partial_update(
        HashMap::new(),
        HashMap::from([(
            "default/a".to_string(),
            resource("a", "https://a-new.example.com", Some(b"a-key")),
        )]),
        HashSet::from(["default/b".to_string()]),
    );

    let resolved = snapshot.get_many_by_key(["default/b", "default/missing", "default/a"]);
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved[0].as_ref().unwrap().endpoint.domain, "b-old.example.com");
    assert!(resolved[1].is_none());
    assert_eq!(resolved[2].as_ref().unwrap().endpoint.domain, "a-old.example.com");
    assert_eq!(
        store.get_by_key("default/a").unwrap().endpoint.domain,
        "a-new.example.com"
    );
    assert!(store.get_by_key("default/b").is_none());
}

#[test]
fn equivalent_resync_reuses_runtime_service_arc() {
    let store = AiBackendStore::new();
    let first = resource("provider", "https://ai.example.com/v1", Some(b"key"));
    store.full_set(&HashMap::from([("default/provider".to_string(), first)]));
    let before = store.get_by_key("default/provider").unwrap();

    // A trailing slash normalizes to the same compiled path prefix.
    let equivalent = resource("provider", "https://ai.example.com/v1/", Some(b"key"));
    store.full_set(&HashMap::from([("default/provider".to_string(), equivalent)]));
    let after = store.get_by_key("default/provider").unwrap();
    assert!(Arc::ptr_eq(&before, &after));
    assert!(changed_ai_backend_targets(
        &HashMap::from([("default/provider".to_string(), before)]),
        &HashMap::from([("default/provider".to_string(), Arc::clone(&after))]),
    )
    .is_empty());

    let changed = resource("provider", "https://changed.example.com/v1", Some(b"key"));
    store.full_set(&HashMap::from([("default/provider".to_string(), changed)]));
    let changed = store.get_by_key("default/provider").unwrap();
    assert!(!Arc::ptr_eq(&after, &changed));
    assert_eq!(
        changed_ai_backend_targets(
            &HashMap::from([("default/provider".to_string(), after)]),
            &HashMap::from([("default/provider".to_string(), changed)]),
        ),
        vec![PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        }]
    );
}

#[test]
fn adaptive_target_changes_include_service_additions_and_removals() {
    let resource = resource("provider", "https://ai.example.com/v1", Some(b"key"));
    let backend = Arc::new(compile_backend("default/provider", &resource).expect("backend fixture must compile"));
    let empty = RuntimeMap::new();
    let populated = RuntimeMap::from([("default/provider".to_string(), backend)]);
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "provider".to_string(),
    };

    assert_eq!(changed_ai_backend_targets(&empty, &populated), vec![target.clone()]);
    assert_eq!(changed_ai_backend_targets(&populated, &empty), vec![target]);
}

#[test]
fn removing_a_service_drops_its_credential_outlier_entry() {
    // The entry is only reachable through a live backend, so leaving one
    // behind is a pure leak — and it is what the integration suite's
    // post-wipe-all residue check catches.
    let store = AiBackendStore::new();
    let key = "default/provider";
    store.full_set(&HashMap::from([(
        key.to_string(),
        resource("provider", "https://ai.example.com", Some(b"key")),
    )]));
    assert_eq!(store.credential_outliers.count(), 1);

    store.full_set(&HashMap::new());
    assert_eq!(
        store.credential_outliers.count(),
        0,
        "credential outlier entries survived their backend: {:?}",
        store.credential_outliers.keys()
    );
}

#[test]
fn an_invalid_update_also_drops_the_entry() {
    // A resource that fails to compile is removed from the runtime map, so
    // its outlier entry must go too — otherwise a permanently broken
    // resource leaks one forever.
    let store = AiBackendStore::new();
    let key = "default/provider";
    store.full_set(&HashMap::from([(
        key.to_string(),
        resource("provider", "https://ai.example.com", Some(b"key")),
    )]));
    assert_eq!(store.credential_outliers.count(), 1);

    // No credential material => compile fails => deactivated.
    store.full_set(&HashMap::from([(
        key.to_string(),
        resource("provider", "https://ai.example.com", None),
    )]));
    assert!(store.get_by_key(key).is_none());
    assert_eq!(store.credential_outliers.count(), 0);
}

#[test]
fn credential_publish_remove_hides_new_capture_but_keeps_old_handle_alive() {
    let store = AiBackendStore::new();
    let key = "default/provider";
    store.full_set(&HashMap::from([(
        key.to_string(),
        resource("provider", "https://ai.example.com", Some(b"key")),
    )]));
    let target = AiBackendStore::target_key(key).unwrap();
    let old_snapshot = store.snapshot();
    let generation = old_snapshot
        .credential_outlier_handle(&target)
        .expect("default generation");
    let captured = generation.capture("primary").expect("published credential");

    store.full_set(&HashMap::new());
    let removed_snapshot = store.snapshot();
    assert!(removed_snapshot.get_by_key(key).is_none());
    assert!(removed_snapshot.credential_outlier_handle(&target).is_none());
    assert!(Arc::ptr_eq(
        &generation,
        &old_snapshot
            .credential_outlier_handle(&target)
            .expect("old snapshot fallback")
    ));
    assert!(matches!(
        captured.admit(),
        crate::backends::outlier_detection::EjectionAdmission::Closed
    ));
}

#[test]
fn backend_snapshots_keep_copy_on_write_fallback_alias_views() {
    let store = AiBackendStore::new();
    let key = "default/provider";
    let target = AiBackendStore::target_key(key).unwrap();
    store.full_set(&HashMap::from([(
        key.to_string(),
        resource("provider", "https://ai.example.com", Some(b"key")),
    )]));
    let first_snapshot = store.snapshot();
    let first = first_snapshot
        .credential_outlier_handle(&target)
        .expect("first fallback");
    let first_primary = first.capture("primary").expect("primary credential");
    first_primary.apply_retry_after("30", std::time::SystemTime::UNIX_EPOCH);

    let mut updated = resource("provider", "https://ai.example.com/v2", Some(b"key"));
    {
        let ai = &mut Arc::get_mut(&mut updated).unwrap().spec.ai;
        let mut credential = ai.credential_pool.credentials[0].clone();
        credential.name = "secondary".to_string();
        ai.credential_pool.credentials.push(credential);
    }
    store.full_set(&HashMap::from([(key.to_string(), updated)]));

    let second_snapshot = store.snapshot();
    let second = second_snapshot
        .credential_outlier_handle(&target)
        .expect("second fallback");
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(first.capture("secondary").is_none());
    assert!(second.capture("secondary").is_some());
    assert!(matches!(
        second.capture("primary").expect("retained primary").admit(),
        crate::backends::outlier_detection::EjectionAdmission::Rejected
    ));

    store.full_set(&HashMap::new());
    assert!(store.snapshot().credential_outlier_handle(&target).is_none());
    assert!(first_snapshot
        .credential_outlier_handle(&target)
        .is_some_and(|handle| Arc::ptr_eq(&handle, &first)));
    assert!(second_snapshot
        .credential_outlier_handle(&target)
        .is_some_and(|handle| Arc::ptr_eq(&handle, &second)));
    assert!(first.capture("primary").is_some());
    assert!(second.capture("secondary").is_some());
}

#[test]
fn runtime_identity_prefers_kubernetes_uid_and_uid_change_breaks_equivalence() {
    let store = AiBackendStore::new();
    let mut first = resource("provider", "https://ai.example.com/v1", Some(b"key"));
    Arc::get_mut(&mut first).unwrap().metadata.uid = Some("uid-generation-a".to_string());
    store.full_set(&HashMap::from([("default/provider".to_string(), first)]));
    let before = store.get_by_key("default/provider").unwrap();
    assert_eq!(before.uid, "uid-generation-a");

    let mut recreated = resource("provider", "https://ai.example.com/v1", Some(b"key"));
    Arc::get_mut(&mut recreated).unwrap().metadata.uid = Some("uid-generation-b".to_string());
    store.full_set(&HashMap::from([("default/provider".to_string(), recreated)]));
    let after = store.get_by_key("default/provider").unwrap();
    assert_eq!(after.uid, "uid-generation-b");
    assert!(!Arc::ptr_eq(&before, &after));
}

#[test]
fn endpoint_root_and_ipv6_authority_are_compiled_once() {
    let root = compile_endpoint(&Url::parse("https://example.com/").unwrap()).unwrap();
    assert!(root.path_prefix.is_empty());
    assert_eq!(root.host, "example.com");

    let ipv6 = compile_endpoint(&Url::parse("https://[2001:db8::1]:8443/base/").unwrap()).unwrap();
    assert_eq!(ipv6.domain, "2001:db8::1");
    assert_eq!(ipv6.host, "[2001:db8::1]:8443");
    assert_eq!(ipv6.path_prefix, "/base");
}

#[test]
fn debug_never_exposes_credential() {
    let backend = try_compile_backend(
        "default/provider",
        &resource(
            "provider",
            "https://ai.example.com",
            Some(b"credential-must-not-appear"),
        ),
    )
    .unwrap();
    let debug = format!("{backend:?}");
    assert!(!debug.contains("credential-must-not-appear"));
    assert!(!debug.contains("credential:"));
    assert!(debug.contains("ai.example.com"));
}

#[test]
fn unresolved_tpm_is_retained_in_the_same_backend_generation() {
    let store = AiBackendStore::new();
    let original = resource("provider", "https://ai.example.com", Some(b"usable-key"));
    store.full_set(&HashMap::from([(
        "default/provider".to_string(),
        Arc::clone(&original),
    )]));
    let before = store.snapshot();
    assert!(!before.get_by_key("default/provider").unwrap().has_credential_tpm);
    let mut updated = (*original).clone();
    let mut unresolved = updated.spec.ai.credential_pool.credentials[0].clone();
    unresolved.name = "unresolved-tpm".to_string();
    unresolved.weight = 1;
    unresolved.credential.secret = None;
    unresolved.limits = Some(AiCredentialLimits {
        requests_per_minute: None,
        tokens_per_minute: Some(1000),
    });
    updated.spec.ai.credential_pool.redis_ref = Some("default/accounting".to_string());
    updated.spec.ai.credential_pool.credentials.push(unresolved);
    store.partial_update(
        HashMap::new(),
        HashMap::from([("default/provider".to_string(), Arc::new(updated))]),
        HashSet::new(),
    );
    let after = store.snapshot();
    let compiled = after.get_by_key("default/provider").unwrap();
    assert!(compiled.has_credential_tpm);
    assert_eq!(compiled.credential_pool.credentials.len(), 1);
    assert!(after.generation() > before.generation());
    assert!(!before.get_by_key("default/provider").unwrap().has_credential_tpm);
}

#[test]
fn unresolved_binding_changes_backend_generation_without_replacing_usable_limiter() {
    let store = AiBackendStore::new();
    let mut configured = (*resource("provider", "https://provider.example.com", Some(b"key"))).clone();
    store.full_set(&HashMap::from([(
        "default/provider".to_string(),
        Arc::new(configured.clone()),
    )]));
    let before = store.snapshot();
    let first = before.get_by_key("default/provider").unwrap();
    assert_eq!(first.declared_credential_count, 1);
    let mut missing = configured.spec.ai.credential_pool.credentials[0].clone();
    missing.name = "missing".to_string();
    missing.weight = 1;
    missing.credential.secret = None;
    configured.spec.ai.credential_pool.credentials.push(missing);
    store.partial_update(
        HashMap::new(),
        HashMap::from([("default/provider".to_string(), Arc::new(configured.clone()))]),
        HashSet::new(),
    );
    let second = store.get_by_key("default/provider").unwrap();
    assert_eq!(second.declared_credential_count, 2);
    assert_eq!(second.credential_pool.credentials.len(), 1);
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(
        &first.credential_pool.limiter_core,
        &second.credential_pool.limiter_core
    ));
    assert_eq!(
        before.get_by_key("default/provider").unwrap().declared_credential_count,
        1
    );
    configured.spec.ai.credential_pool.credentials.pop();
    store.partial_update(
        HashMap::new(),
        HashMap::from([("default/provider".to_string(), Arc::new(configured))]),
        HashSet::new(),
    );
    assert_eq!(
        store.get_by_key("default/provider").unwrap().declared_credential_count,
        1
    );
}
