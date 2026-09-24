use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::{raw_store::RawStore, KubeRawStore};
use k8s_openapi::api::core::v1::ConfigMap;

fn object(ns: &str, name: &str) -> ConfigMap {
    let mut obj = ConfigMap::default();
    obj.metadata.name = Some(name.into());
    obj.metadata.namespace = Some(ns.into());
    obj
}

#[test]
fn restarted_generations_preserve_committed_union_and_teardown_clears_retained_readers() {
    let mut a = SourceState::new(WatchScope::Namespace("a".into()));
    let mut b = SourceState::new(WatchScope::Namespace("b".into()));
    let raw = KubeRawStore::namespaced(std::collections::HashMap::from([
        ("a".into(), a.store.clone()),
        ("b".into(), b.store.clone()),
    ]));
    a.apply(Event::Init).unwrap();
    a.apply(Event::InitApply(object("a", "abandoned"))).unwrap();
    assert!(raw.snapshot().is_empty());
    a.apply(Event::Init).unwrap();
    a.apply(Event::InitApply(object("a", "old"))).unwrap();
    a.apply(Event::InitDone).unwrap();
    let old = Arc::downgrade(&raw.get("a/old").unwrap());
    a.apply(Event::Init).unwrap();
    a.apply(Event::InitApply(object("a", "new"))).unwrap();
    b.apply(Event::Init).unwrap();
    b.apply(Event::InitApply(object("b", "kept"))).unwrap();
    b.apply(Event::InitDone).unwrap();
    assert!(a.ever_committed && b.ever_committed);
    assert!(raw.get("a/old").is_some());
    assert!(raw.get("a/new").is_none());
    assert!(raw.get("a/abandoned").is_none());
    let keys = a.apply(Event::InitDone).unwrap().into_iter().collect::<HashSet<_>>();
    assert_eq!(keys, HashSet::from(["a/old".into(), "a/new".into()]));
    assert!(old.upgrade().is_none());
    assert!(raw.get("b/kept").is_some());
    a.apply(Event::InitDone).unwrap();
    assert!(raw.get("a/new").is_some());
    a.apply(Event::Init).unwrap();
    a.apply(Event::InitDone).unwrap();
    assert_eq!(raw.snapshot().len(), 1);
    b.apply(Event::Init).unwrap();
    b.apply(Event::InitApply(object("b", "unfinished"))).unwrap();
    drop((a, b));
    assert!(raw.snapshot().is_empty());
}

use std::sync::Arc;

#[derive(Clone, Debug)]
struct Tracked {
    metadata: kube::api::ObjectMeta,
    _lifetime: Arc<()>,
}
impl Resource for Tracked {
    type DynamicType = ();
    type Scope = kube::core::ClusterResourceScope;
    fn kind(_: &()) -> std::borrow::Cow<'static, str> {
        "Tracked".into()
    }
    fn group(_: &()) -> std::borrow::Cow<'static, str> {
        "test.example.com".into()
    }
    fn version(_: &()) -> std::borrow::Cow<'static, str> {
        "v1".into()
    }
    fn plural(_: &()) -> std::borrow::Cow<'static, str> {
        "trackeds".into()
    }
    fn meta(&self) -> &kube::api::ObjectMeta {
        &self.metadata
    }
    fn meta_mut(&mut self) -> &mut kube::api::ObjectMeta {
        &mut self.metadata
    }
}

#[test]
fn staging_and_committed_objects_release_even_with_retained_diagnostic_readers() {
    let lifetime = Arc::new(());
    let object = || Tracked {
        metadata: kube::api::ObjectMeta {
            name: Some("tracked".into()),
            ..Default::default()
        },
        _lifetime: lifetime.clone(),
    };
    let mut source = SourceState::new(WatchScope::Cluster);
    let retained = source.store.clone();
    source.apply(Event::Init).unwrap();
    source.apply(Event::InitApply(object())).unwrap();
    assert_eq!(Arc::strong_count(&lifetime), 2);
    source.apply(Event::Init).unwrap();
    assert_eq!(Arc::strong_count(&lifetime), 1);
    source.apply(Event::InitApply(object())).unwrap();
    source.apply(Event::InitDone).unwrap();
    source.apply(Event::Init).unwrap();
    source.apply(Event::InitApply(object())).unwrap();
    assert_eq!(Arc::strong_count(&lifetime), 3);
    drop(source);
    assert!(retained.state().is_empty());
    assert_eq!(Arc::strong_count(&lifetime), 1);
}

#[test]
fn malformed_identity_and_transitions_never_commit() {
    let mut source = SourceState::<ConfigMap>::new(WatchScope::Namespace("a".into()));
    assert!(source.apply(Event::Apply(object("a", "x"))).is_err());
    assert!(source.apply(Event::InitApply(object("a", "x"))).is_err());
    source.apply(Event::Init).unwrap();
    assert!(source.apply(Event::InitApply(object("b", "x"))).is_err());
    assert!(source.apply(Event::InitApply(object("a", ""))).is_err());
    assert!(source.apply(Event::Delete(object("a", "x"))).is_err());
    assert!(!source.ever_committed);
    source.apply(Event::InitDone).unwrap();
    assert!(source.ever_committed);
    assert!(source.store.state().is_empty());
}
