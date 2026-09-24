use super::*;

#[derive(Clone, Debug, PartialEq)]
struct Dummy(String);

#[test]
fn local_store_snapshot_reflects_upserts_and_removes() {
    let store: LocalRawStore<Dummy> = LocalRawStore::new();
    assert!(store.snapshot().is_empty());

    store.upsert("ns1/a".to_string(), Arc::new(Dummy("a".into())));
    store.upsert("ns1/b".to_string(), Arc::new(Dummy("b".into())));

    let snap = store.snapshot();
    assert_eq!(snap.len(), 2);
    let mut names: Vec<String> = snap.iter().map(|a| a.0.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);

    store.remove("ns1/a");
    let snap = store.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].0, "b");
}
