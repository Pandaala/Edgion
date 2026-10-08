use super::*;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry};
use edgion_resources::resources::http_route::HTTPHeaderFilter;

fn create_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
    EdgionPlugins {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
                HTTPHeaderFilter {
                    set: None,
                    add: None,
                    remove: Some(vec!["X-Remove".to_string()]),
                },
            ))]),
            ..Default::default()
        },
        status: None,
    }
}

fn create_invalid_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
    EdgionPlugins {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
                HTTPHeaderFilter {
                    set: None,
                    add: None,
                    remove: None,
                },
            ))]),
            ..Default::default()
        },
        status: None,
    }
}

fn create_invalid_dsl_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
    use edgion_resources::resources::edgion_plugins::DslConfig;

    EdgionPlugins {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                name: "invalid-reload".to_string(),
                source: Some("let value =".to_string()),
                ..Default::default()
            }))]),
            ..Default::default()
        },
        status: None,
    }
}

#[test]
fn test_full_set() {
    let store = PluginStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/plugin1".to_string(),
        Arc::new(create_test_plugin("default", "plugin1")),
    );
    data.insert(
        "default/plugin2".to_string(),
        Arc::new(create_test_plugin("default", "plugin2")),
    );

    store.full_set(&data);

    assert!(store.contains("default/plugin1"));
    assert!(store.contains("default/plugin2"));
    assert!(!store.contains("default/plugin3"));
}

#[test]
fn test_partial_update_add() {
    let store = PluginStore::new();

    let mut add = HashMap::new();
    add.insert(
        "default/plugin1".to_string(),
        Arc::new(create_test_plugin("default", "plugin1")),
    );

    store.partial_update(add, HashMap::new(), HashSet::new());

    assert!(store.contains("default/plugin1"));
}

#[test]
fn test_partial_update_update() {
    let store = PluginStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/plugin1".to_string(),
        Arc::new(create_test_plugin("default", "plugin1")),
    );
    store.full_set(&data);

    let mut update = HashMap::new();
    update.insert(
        "default/plugin1".to_string(),
        Arc::new(create_test_plugin("default", "plugin1")),
    );

    store.partial_update(HashMap::new(), update, HashSet::new());

    assert!(store.contains("default/plugin1"));
}

#[test]
fn test_partial_update_remove() {
    let store = PluginStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/plugin1".to_string(),
        Arc::new(create_test_plugin("default", "plugin1")),
    );
    store.full_set(&data);

    let mut remove = HashSet::new();
    remove.insert("default/plugin1".to_string());
    store.partial_update(HashMap::new(), HashMap::new(), remove);

    assert!(!store.contains("default/plugin1"));
}

#[test]
fn invalid_partial_update_retains_last_known_good() {
    let store = PluginStore::new();
    store.partial_update(
        HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    let previous_runtime = store
        .get_plugin_runtime("default/plugin1")
        .expect("valid plugin runtime should be stored");

    store.partial_update(
        HashMap::new(),
        HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_invalid_dsl_test_plugin("default", "plugin1")),
        )]),
        HashSet::new(),
    );

    assert_eq!(
        store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
        Some(true)
    );
    let retained_runtime = store
        .get_plugin_runtime("default/plugin1")
        .expect("last-known-good runtime should be retained");
    assert!(Arc::ptr_eq(
        previous_runtime.request_plugins_iter().next().unwrap(),
        retained_runtime.request_plugins_iter().next().unwrap(),
    ));
}

#[test]
fn invalid_same_key_add_replaces_last_known_good_and_fails_closed() {
    let store = PluginStore::new();
    store.partial_update(
        HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    let previous_runtime = store
        .get_plugin_runtime("default/plugin1")
        .expect("valid plugin runtime should be stored");

    store.partial_update(
        HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_invalid_dsl_test_plugin("default", "plugin1")),
        )]),
        HashMap::new(),
        HashSet::new(),
    );

    assert_eq!(
        store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
        Some(false)
    );
    let replacement_runtime = store
        .get_plugin_runtime("default/plugin1")
        .expect("invalid add should publish a fail-closed runtime");
    assert!(!Arc::ptr_eq(&previous_runtime, &replacement_runtime));
}

#[test]
fn invalid_full_snapshot_retains_last_known_good_but_drops_absent_keys() {
    let store = PluginStore::new();
    store.full_set(&HashMap::from([
        (
            "default/kept".to_string(),
            Arc::new(create_test_plugin("default", "kept")),
        ),
        (
            "default/removed".to_string(),
            Arc::new(create_test_plugin("default", "removed")),
        ),
    ]));
    let previous_runtime = store
        .get_plugin_runtime("default/kept")
        .expect("valid plugin runtime should be stored");

    store.full_set(&HashMap::from([(
        "default/kept".to_string(),
        Arc::new(create_invalid_dsl_test_plugin("default", "kept")),
    )]));

    assert_eq!(
        store.with_plugin("default/kept", |compiled| compiled.is_activatable),
        Some(true)
    );
    let retained_runtime = store
        .get_plugin_runtime("default/kept")
        .expect("last-known-good runtime should be retained");
    assert!(Arc::ptr_eq(
        previous_runtime.request_plugins_iter().next().unwrap(),
        retained_runtime.request_plugins_iter().next().unwrap(),
    ));
    assert!(!store.contains("default/removed"));
}

#[test]
fn invalid_initial_resource_remains_fail_closed() {
    let store = PluginStore::new();
    store.full_set(&HashMap::from([(
        "default/plugin1".to_string(),
        Arc::new(create_invalid_test_plugin("default", "plugin1")),
    )]));

    assert_eq!(
        store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
        Some(false)
    );
}

#[test]
fn non_dsl_invalid_update_replaces_last_known_good_with_fail_closed_runtime() {
    let store = PluginStore::new();
    store.full_set(&HashMap::from([(
        "default/plugin1".to_string(),
        Arc::new(create_test_plugin("default", "plugin1")),
    )]));

    store.partial_update(
        HashMap::new(),
        HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_invalid_test_plugin("default", "plugin1")),
        )]),
        HashSet::new(),
    );

    assert_eq!(
        store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
        Some(false)
    );
}

#[test]
fn raw_delta_keys_notify_semantic_targets_without_aliasing_empty_namespace() {
    let keys = ["plugins", "default/plugins", "other/plugins", "/plugins"].map(str::to_owned);
    assert_eq!(
        super::changed_plugin_targets(keys.iter()),
        HashSet::from(["default/plugins".to_owned(), "other/plugins".to_owned()])
    );
}
