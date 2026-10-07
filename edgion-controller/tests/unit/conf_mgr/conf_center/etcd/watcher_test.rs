use super::*;
use edgion_common::link_sys_core::providers::etcd::EtcdBootstrapConfig;

fn watcher_without_connection() -> EtcdWatcher {
    let config = EtcdBootstrapConfig {
        endpoints: vec!["http://127.0.0.1:2379".into()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
    };
    let client = EtcdLinkClient::from_bootstrap("test/retention", config).unwrap();
    EtcdWatcher::new(Arc::new(client), "/edgion")
}

#[tokio::test]
async fn init_documents_release_per_subscribed_phase() {
    let watcher = watcher_without_connection();
    let first_key = "/edgion/conf/Secret/default/first".to_owned();
    let later_key = "/edgion/conf/EdgionPlugins/default/later".to_owned();
    let first_document = "first document".to_owned();
    let first_pointer = first_document.as_ptr();
    let later_document = "later document".to_owned();
    *watcher.primed.write() = Some(PrimedSnapshot {
        by_kind: HashMap::from([
            ("Secret", vec![(first_key.clone(), first_document)]),
            ("EdgionPlugins", vec![(later_key.clone(), later_document.clone())]),
        ]),
        revision: 42,
    });
    let mut first = watcher.subscribe("Secret").await;
    let mut empty = watcher.subscribe("Gateway").await;
    watcher
        .dispatch_init(&["Secret", "EdgionPlugins", "Gateway"])
        .await
        .unwrap();
    assert!(matches!(first.recv().await.unwrap(), EtcdEvent::Init));
    match first.recv().await.unwrap() {
        EtcdEvent::InitApply { key, content } => {
            assert_eq!(key, first_key);
            assert_eq!(content, "first document");
            assert_eq!(content.as_ptr(), first_pointer);
        }
        event => panic!("unexpected event: {event:?}"),
    }
    assert!(matches!(first.recv().await.unwrap(), EtcdEvent::InitDone));
    assert!(matches!(empty.recv().await.unwrap(), EtcdEvent::Init));
    assert!(matches!(empty.recv().await.unwrap(), EtcdEvent::InitDone));
    {
        let guard = watcher.primed.read();
        let snapshot = guard.as_ref().unwrap();
        assert!(!snapshot.by_kind.contains_key("Secret"));
        assert_eq!(snapshot.by_kind["EdgionPlugins"][0].1, later_document);
        assert_eq!(snapshot.revision, 42);
    }
    let mut later = watcher.subscribe("EdgionPlugins").await;
    watcher.dispatch_init(&["EdgionPlugins"]).await.unwrap();
    assert!(matches!(later.recv().await.unwrap(), EtcdEvent::Init));
    match later.recv().await.unwrap() {
        EtcdEvent::InitApply { key, content } => {
            assert_eq!(key, later_key);
            assert_eq!(content, later_document);
        }
        event => panic!("unexpected event: {event:?}"),
    }
    assert!(matches!(later.recv().await.unwrap(), EtcdEvent::InitDone));
    assert!(watcher.primed.read().as_ref().unwrap().by_kind.is_empty());
}

#[test]
fn runtime_releases_documents_and_preserves_revision_and_known_keys() {
    let watcher = watcher_without_connection();
    assert!(watcher.release_primed_documents().is_err());
    let key = "/edgion/conf/Secret/default/remaining".to_owned();
    *watcher.primed.write() = Some(PrimedSnapshot {
        by_kind: HashMap::from([("Secret", vec![(key.clone(), "document".into())])]),
        revision: 42,
    });
    watcher
        .known_keys
        .write()
        .insert("Secret", HashSet::from([key.clone()]));
    assert_eq!(watcher.release_primed_documents().unwrap(), 42);
    assert!(watcher.primed.read().as_ref().unwrap().by_kind.is_empty());
    assert_eq!(watcher.primed.read().as_ref().unwrap().revision, 42);
    assert!(watcher.known_keys.read()["Secret"].contains(&key));
}
