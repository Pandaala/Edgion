use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountedVeto(Arc<AtomicUsize>);

impl UpstreamResponseFilter for CountedVeto {
    fn name(&self) -> &str {
        "C0CountedVeto"
    }

    fn run_upstream_response_filter(&self, session: &mut dyn PluginSession, _: &mut PluginLog) -> PluginRunningResult {
        self.0.fetch_add(1, Ordering::AcqRel);
        session.set_response_header("cache-control", "no-store").unwrap();
        PluginRunningResult::GoodNext
    }
}

impl PluginRuntime {
    pub(crate) fn test_add_counted_veto(&mut self, count: Arc<AtomicUsize>) {
        self.upstream_response_plugins.push(Arc::new(CountedVeto(count)));
    }
}

#[test]
fn response_cache_actual_construction_cache_reuse_preserves_certificates_and_runtime_ids() {
    let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "cache-cert", "name": "plugins"},
        "spec": {"requestPlugins": [
            {"type": "ResponseCache", "config": {}},
            {"type": "RequestHeaderModifier", "config": {"set": [{"name":"x-public", "value":"yes"}]}}
        ], "upstreamResponseFilterPlugins": [
            {"type": "ResponseHeaderModifier", "config": {"set": [{"name":"x-response", "value":"yes"}]}}
        ]}
    }))
    .unwrap();
    let mut cache = super::super::entry_bundle::EntryCache::default();
    let mut first = PluginRuntime::new();
    assert!(first.add_resource_filters_with_cache(&owner, &mut cache).is_empty());
    assert!(first.cache_safe());
    let mut reused = PluginRuntime::new();
    assert!(reused.add_resource_filters_with_cache(&owner, &mut cache).is_empty());
    assert!(reused.cache_safe());
    assert_ne!(first.cache_identity(), reused.cache_identity());
    assert!(first.cache_identity() > 0);
    assert!(Arc::ptr_eq(&first.request_plugins[0], &reused.request_plugins[0]));
    assert!(Arc::ptr_eq(
        &first.upstream_response_plugins[0],
        &reused.upstream_response_plugins[0]
    ));
    let mut unsafe_owner = owner.clone();
    unsafe_owner.spec.upstream_response_filter_plugins = Some(vec![serde_json::from_value(serde_json::json!({
        "type":"Dsl", "config":{"name":"response-dsl", "source":"let a = 1"}
    }))
    .unwrap()]);
    let mut unsafe_runtime = PluginRuntime::new();
    assert!(unsafe_runtime
        .add_resource_filters_with_cache(&unsafe_owner, &mut super::super::entry_bundle::EntryCache::default())
        .is_empty());
    assert!(!unsafe_runtime.cache_safe());
}
