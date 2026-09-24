use super::*;
use edgion_resources::resources::edgion_plugins::plugin_configs::access_log_extern::{
    AccessLogExternField, LogExtSource,
};
use edgion_resources::resources::edgion_plugins::EdgionPluginsSpec;
use edgion_resources::resources::EdgionPlugins;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Barrier;

fn ep_without_access_log_extern() -> EdgionPlugins {
    EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec::default(),
        status: None,
    }
}

fn ep_with_access_log_extern() -> EdgionPlugins {
    EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            access_log_extern: Some(vec![AccessLogExternField {
                key: "tenant".to_string(),
                from: LogExtSource::Header,
                name: "x-tenant".to_string(),
            }]),
            ..Default::default()
        },
        status: None,
    }
}

fn assert_snapshot_coherent(store: &PluginStore) {
    let snapshot = store.publisher().snapshot();
    let gateway = snapshot.gateway("plugin-store-test/scope").unwrap();
    assert_eq!(
        gateway.scope.has_access_log_extern,
        gateway
            .scope
            .global_keys
            .iter()
            .filter_map(|key| gateway.scope.http(key))
            .any(|view| view
                .spec
                .spec
                .access_log_extern
                .as_ref()
                .is_some_and(|fields| !fields.is_empty()))
    );
}

/// The flag must reflect what is currently *stored*, not merely what
/// entered the store historically: no plugins, plugins without
/// `accessLogExtern`, then adding one, then removing it.
#[test]
fn flag_flips_as_the_store_gains_and_loses_an_access_log_extern_declaration() {
    let store = PluginStore::new();
    assert!(!store.scope().has_access_log_extern, "empty store must report false");
    assert_snapshot_coherent(&store);

    let mut plugins = HashMap::new();
    plugins.insert(
        "ns/no-ext".to_string(),
        compile_plugins(&ep_without_access_log_extern()),
    );
    store.replace_all(plugins);
    assert!(
        !store.scope().has_access_log_extern,
        "a store with plugins but no accessLogExtern must report false"
    );
    assert_snapshot_coherent(&store);

    // Deterministic false-to-true transition.
    let mut add = HashMap::new();
    add.insert("ns/with-ext".to_string(), compile_plugins(&ep_with_access_log_extern()));
    store.apply_delta(add, HashMap::new(), &HashSet::new());
    assert!(
        store.scope().has_access_log_extern,
        "adding a resource with accessLogExtern must flip the flag to true"
    );
    assert_snapshot_coherent(&store);

    // Deterministic true-to-false transition.
    let mut remove = HashSet::new();
    remove.insert("ns/with-ext".to_string());
    store.apply_delta(HashMap::new(), HashMap::new(), &remove);
    assert!(
        !store.scope().has_access_log_extern,
        "removing the only resource with accessLogExtern must flip the flag back to false"
    );
    assert_snapshot_coherent(&store);
}

/// `replace_all` is the conf_sync full-sync path: it must SET the flag, not
/// merely leave it. Without this the `new()` default (false) masks a missing
/// store in `replace_all`.
#[test]
fn replace_all_sets_flag_based_on_content() {
    let store = PluginStore::new();

    // replace_all with a resource declaring accessLogExtern must set the flag.
    let mut full = HashMap::new();
    full.insert("ns/with-ext".to_string(), compile_plugins(&ep_with_access_log_extern()));
    store.replace_all(full);
    assert!(
        store.scope().has_access_log_extern,
        "replace_all with an accessLogExtern resource must set the flag"
    );

    // replace_all with an empty set must clear the flag.
    store.replace_all(HashMap::new());
    assert!(
        !store.scope().has_access_log_extern,
        "replace_all with an empty set must clear the flag"
    );
}

#[test]
fn concurrent_publication_never_exposes_a_mixed_snapshot() {
    const READERS: usize = 4;
    const WRITES: usize = 2_000;
    const READS: usize = 10_000;

    let with_ext = compile_plugins(&ep_with_access_log_extern());
    let without_ext = compile_plugins(&ep_without_access_log_extern());
    let store = Arc::new(PluginStore::new());
    let start = Arc::new(Barrier::new(READERS + 1));
    let failed = Arc::new(AtomicBool::new(false));

    std::thread::scope(|scope| {
        {
            let store = Arc::clone(&store);
            let start = Arc::clone(&start);
            let with_ext = Arc::clone(&with_ext);
            let without_ext = Arc::clone(&without_ext);
            scope.spawn(move || {
                start.wait();
                for iteration in 0..WRITES {
                    let generation = if iteration % 2 == 0 {
                        HashMap::from([("ns/with-ext".to_string(), Arc::clone(&with_ext))])
                    } else {
                        HashMap::from([("ns/without-ext".to_string(), Arc::clone(&without_ext))])
                    };
                    store.replace_all(generation);
                }
            });
        }

        for _ in 0..READERS {
            let store = Arc::clone(&store);
            let start = Arc::clone(&start);
            let failed = Arc::clone(&failed);
            scope.spawn(move || {
                start.wait();
                for _ in 0..READS {
                    let snapshot = store.publisher().snapshot();
                    let gateway = snapshot.gateway("plugin-store-test/scope").unwrap();
                    let declares = gateway
                        .scope
                        .global_keys
                        .iter()
                        .filter_map(|key| gateway.scope.http(key))
                        .any(|view| {
                            view.spec
                                .spec
                                .access_log_extern
                                .as_ref()
                                .is_some_and(|fields| !fields.is_empty())
                        });
                    if gateway.scope.has_access_log_extern != declares {
                        failed.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    assert!(
        !failed.load(Ordering::Relaxed),
        "each reader must observe a coherent plugin-store generation"
    );
}
