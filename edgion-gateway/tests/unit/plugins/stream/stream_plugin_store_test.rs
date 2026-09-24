use super::*;
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamLayer, StreamPluginResult};
use edgion_resources::resource::meta::ResourceMeta;
use std::net::IpAddr;

fn local_limit_resource(name: &str, rate: u64, enabled: bool) -> Arc<EdgionStreamPlugins> {
    let yaml = format!(
        r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata: {{name: {name}, namespace: default}}
spec:
  plugins:
    - type: ConnectionRateLimit
      enable: {enabled}
      config:
        perListener: {{rate: {rate}, interval: 1h}}
"#
    );
    Arc::new(serde_yaml::from_str(&yaml).unwrap())
}

#[tokio::test]
async fn local_budget_survives_full_delta_delete_and_disabled_generations() {
    let store = StreamPluginStore::new();
    let name = "local-budget-store-reload";
    let key = format!("default/{name}");
    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 18089, StreamLayer::Stage1);
    store.full_set(&HashMap::from([(key.clone(), local_limit_resource(name, 2, true))]));
    let old = store.get(&key).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            old.stream_plugin_runtime.run(&ctx).await,
            StreamPluginResult::Allow
        ));
    }
    assert!(matches!(
        old.stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Deny(_)
    ));

    store.full_set(&HashMap::from([(key.clone(), local_limit_resource(name, 2, true))]));
    assert!(matches!(
        store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Deny(_)
    ));

    store.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), local_limit_resource(name, 4, true))]),
        HashSet::new(),
    );
    let raised = store.get(&key).unwrap();
    assert!(matches!(
        raised.stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        old.stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        raised.stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Deny(_)
    ));

    store.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
    assert!(store.get(&key).is_none());
    assert!(matches!(
        old.stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Deny(_)
    ));
    store.partial_update(
        HashMap::from([(key.clone(), local_limit_resource(name, 4, true))]),
        HashMap::new(),
        HashSet::new(),
    );
    assert!(matches!(
        store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Deny(_)
    ));

    store.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), local_limit_resource(name, 100, false))]),
        HashSet::new(),
    );
    assert!(matches!(
        store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Allow
    ));
    store.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), local_limit_resource(name, 4, true))]),
        HashSet::new(),
    );
    assert!(matches!(
        store.get(&key).unwrap().stream_plugin_runtime.run(&ctx).await,
        StreamPluginResult::Deny(_)
    ));
}

fn fixture(name: &str) -> Arc<EdgionStreamPlugins> {
    let yaml = format!(
        r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: {name}
  namespace: default
spec: {{}}
"#,
    );
    Arc::new(serde_yaml::from_str(&yaml).expect("minimal EdgionStreamPlugins fixture must parse"))
}

#[test]
fn empty_store_reports_empty_breakdown() {
    let store = StreamPluginStore::new();
    let by_kind = store.count_by_kind();
    assert!(by_kind.is_empty());
    let keys = store.keys_by_kind("IpRestriction");
    assert!(keys.is_empty());
}

#[test]
fn full_set_publishes_then_empty_full_set_clears() {
    let store = StreamPluginStore::new();
    let resource = fixture("full-set");
    let key = resource.key_name();
    store.full_set(&HashMap::from([(key.clone(), resource)]));

    let published = store.get(&key).expect("full set must publish the compiled resource");
    assert_eq!(store.count(), 1);
    assert_eq!(published.spec.key_name(), key);

    store.full_set(&HashMap::new());
    assert_eq!(store.count(), 0);
    assert!(!store.contains(&key));
    assert_eq!(published.spec.key_name(), "default/full-set");
}

#[test]
fn partial_delta_preserves_replaces_adds_and_removes_snapshots() {
    let store = StreamPluginStore::new();
    let replace_key = "default/replace".to_string();
    let keep_key = "default/keep".to_string();
    let remove_key = "default/remove".to_string();
    store.full_set(&HashMap::from([
        (replace_key.clone(), fixture("replace")),
        (keep_key.clone(), fixture("keep")),
        (remove_key.clone(), fixture("remove")),
    ]));

    let old_replaced = store.get(&replace_key).expect("replacement predecessor must exist");
    let old_kept = store.get(&keep_key).expect("unaffected entry must exist");
    let mut changed = (*fixture("replace")).clone();
    changed.spec.plugins = Some(Vec::new());
    store.partial_update(
        HashMap::from([("default/add".to_string(), fixture("add"))]),
        HashMap::from([(replace_key.clone(), Arc::new(changed))]),
        HashSet::from([remove_key.clone()]),
    );

    let replacement = store.get(&replace_key).expect("updated entry must remain present");
    let kept = store.get(&keep_key).expect("unaffected entry must remain present");
    assert!(!Arc::ptr_eq(&old_replaced, &replacement));
    assert!(Arc::ptr_eq(&old_kept, &kept));
    assert!(store.contains("default/add"));
    assert!(!store.contains(&remove_key));
    assert_eq!(store.count(), 3);
    assert_eq!(old_replaced.spec.key_name(), replace_key);
}
