use super::*;

#[test]
fn resolution_preserves_metadata_warnings_and_replaces_credentials() {
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{KeyAuthConfig, KeyAuthSecretGroup};
    use edgion_resources::resources::edgion_plugins::{EdgionPluginsSpec, RequestFilterEntry};
    use edgion_resources::resources::gateway::SecretObjectReference;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::io::{Read, Seek};
    use std::sync::Arc;

    let secret = |name: &str, yaml: &str| Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(std::collections::BTreeMap::from([(
            "keys.yaml".into(),
            ByteString(yaml.as_bytes().to_vec()),
        )])),
        ..Default::default()
    };
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(HashMap::from([
            (
                "default/first".into(),
                secret("first", "- apiKey: ' private-key-é '\n  x-owner: first\n  ignored: private-metadata\n- apiKey: ' private-key-é '\n  x-owner: rejected-within\n- x-owner: missing-key\n- apiKey: ''\n  x-owner: empty-key\n"),
            ),
            (
                "default/second".into(),
                secret("second", "- apiKey: ' private-key-é '\n  x-owner: rejected-across\n- apiKey: other-key\n  x-owner: second\n"),
            ),
        ]));
    let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, Arc::clone(&runtime));
    let mut ep = EdgionPlugins::new("key-ownership", EdgionPluginsSpec::default());
    ep.metadata.namespace = Some("default".into());
    ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig {
        key_field: "apiKey".into(),
        upstream_header_fields: vec!["apiKey".into(), "x-owner".into()],
        secret_groups: Some(
            [("group-a", "first"), ("group-b", "second")]
                .into_iter()
                .map(|(group, name)| KeyAuthSecretGroup {
                    name: group.into(),
                    description: None,
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        namespace: None,
                        name: name.into(),
                    }],
                })
                .collect(),
        ),
        ..Default::default()
    }))]);
    let owner = ResourceRef::new(
        edgion_resources::ResourceKind::EdgionPlugins,
        Some("default".into()),
        "key-ownership".into(),
    );
    let mut log_file = tempfile::tempfile().unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log_file.try_clone().unwrap())
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || resolve(&mut ep, &owner, &ctx));
    log_file.rewind().unwrap();
    let mut logs = String::new();
    log_file.read_to_string(&mut logs).unwrap();
    assert!(!logs.contains("private-key"));
    assert!(!logs.contains("private-metadata"));
    let within = logs
        .lines()
        .find(|line| line.contains("Duplicate API key within group"))
        .unwrap();
    assert!(within.contains("secret_key=default/first") && within.contains("group=group-a"));
    let across = logs
        .lines()
        .find(|line| line.contains("duplicate API key across groups"))
        .unwrap();
    assert!(across.contains("existing_group=group-a") && across.contains("incoming_group=group-b"));
    assert!(across.contains("secret_key=default/second"));
    assert!(logs.contains("Key entry missing key field"));

    let EdgionPlugin::KeyAuth(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
        panic!("expected KeyAuth")
    };
    let keys = config.resolved_keys.as_ref().unwrap();
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[" private-key-é "].group, "group-a");
    assert_eq!(
        keys[" private-key-é "].headers,
        HashMap::from([("x-owner".into(), "first".into())])
    );
    assert_eq!(keys[""].headers["x-owner"], "empty-key");
    assert_eq!(keys["other-key"].group, "group-b");
    for name in ["first", "second"] {
        assert_eq!(ctx.secret_ref_manager().get_refs(&format!("default/{name}")).len(), 1);
    }

    runtime.secret_store.replace_all(HashMap::from([(
        "default/first".into(),
        secret("first", "- apiKey: rotated-key\n  x-owner: rotated\n"),
    )]));
    resolve(&mut ep, &owner, &ctx);
    let EdgionPlugin::KeyAuth(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
        panic!("expected KeyAuth")
    };
    let keys = config.resolved_keys.as_ref().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys["rotated-key"].headers["x-owner"], "rotated");
    runtime.secret_store.replace_all(HashMap::new());
    resolve(&mut ep, &owner, &ctx);
    let EdgionPlugin::KeyAuth(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
        panic!("expected KeyAuth")
    };
    assert!(config.resolved_keys.is_none());
}

#[test]
fn first_wins_second_group_rejected() {
    let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

    let group_a: String = "partner-a".to_string();
    let group_b: String = "partner-b".to_string();

    // First group inserts the key — should succeed.
    let inserted = first_wins_insert(&mut all_keys, "dup-key".to_string(), HashMap::new(), &group_a);
    assert!(inserted.is_ok(), "first insert must succeed");

    // Second group tries the same key — must be rejected.
    let inserted_again = first_wins_insert(&mut all_keys, "dup-key".to_string(), HashMap::new(), &group_b);
    assert_eq!(
        inserted_again,
        Err("partner-a"),
        "duplicate must report the retained group"
    );

    // Verify first-wins: the stored group is partner-a, not partner-b.
    assert_eq!(
        all_keys.get("dup-key").unwrap().group.as_str(),
        "partner-a",
        "first group must win"
    );

    assert_eq!(all_keys.len(), 1, "duplicate must not add an entry");
}

#[test]
fn first_wins_distinct_keys_both_inserted() {
    let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

    let group_a: String = "partner-a".to_string();
    let group_b: String = "partner-b".to_string();

    assert!(first_wins_insert(&mut all_keys, "key-a".to_string(), HashMap::new(), &group_a,).is_ok());
    assert!(first_wins_insert(&mut all_keys, "key-b".to_string(), HashMap::new(), &group_b,).is_ok());

    assert_eq!(all_keys.len(), 2);
    assert_eq!(all_keys.get("key-a").unwrap().group.as_str(), "partner-a");
    assert_eq!(all_keys.get("key-b").unwrap().group.as_str(), "partner-b");
}
