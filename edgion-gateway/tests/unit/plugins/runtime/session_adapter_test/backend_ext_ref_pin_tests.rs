use crate::plugins::http::{compile_plugins, PluginStore};
use crate::plugins::runtime::PluginRuntime;
use crate::EdgionHttpContext;
use edgion_resources::resources::EdgionPlugins;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

#[test]
fn pins_present_and_missing_results() {
    let mut ctx = EdgionHttpContext::new();
    let runtime = Arc::new(PluginRuntime::new());
    let first = ctx
        .pinned_backend_ext_ref("ns/present", || Some(Arc::clone(&runtime)))
        .unwrap();
    let second = ctx
        .pinned_backend_ext_ref("ns/present", || panic!("present entry was reloaded"))
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));

    assert!(ctx.pinned_backend_ext_ref("ns/missing", || None).is_none());
    assert!(ctx
        .pinned_backend_ext_ref("ns/missing", || panic!("missing entry was reloaded"))
        .is_none());
}

#[test]
fn pins_plugin_store_generation_across_hot_update() {
    let store = PluginStore::new();
    let key = "ns/credential-provider";
    let generation_a = compile_plugins(&EdgionPlugins::new("credential-provider-a", Default::default()));
    let mut initial = HashMap::new();
    initial.insert(key.to_string(), Arc::clone(&generation_a));
    store.apply_delta(HashMap::new(), initial, &HashSet::new());

    let published_a = store.get_plugin_runtime(key).unwrap();
    let mut ctx = EdgionHttpContext::new();
    let pinned_a = ctx
        .pinned_backend_ext_ref(key, || store.get_plugin_runtime(key))
        .unwrap();
    assert!(Arc::ptr_eq(&pinned_a, &published_a));

    let generation_b = compile_plugins(&EdgionPlugins::new("credential-provider-b", Default::default()));
    let mut update = HashMap::new();
    update.insert(key.to_string(), Arc::clone(&generation_b));
    store.apply_delta(HashMap::new(), update, &HashSet::new());
    let published_b = store.get_plugin_runtime(key).unwrap();
    assert!(!Arc::ptr_eq(&published_a, &published_b));

    let retry_runtime = ctx
        .pinned_backend_ext_ref(key, || store.get_plugin_runtime(key))
        .unwrap();
    assert!(Arc::ptr_eq(&retry_runtime, &published_a));
    assert!(!Arc::ptr_eq(&retry_runtime, &published_b));
}
