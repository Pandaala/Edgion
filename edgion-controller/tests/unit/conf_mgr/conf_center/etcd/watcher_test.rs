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
            ("Secret", vec![(first_key.clone(), EtcdDocument::Text(first_document))]),
            (
                "EdgionPlugins",
                vec![(later_key.clone(), EtcdDocument::Text(later_document.clone()))],
            ),
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
            let EtcdDocument::Text(content) = content else {
                panic!("expected text")
            };
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
        assert_eq!(
            snapshot.by_kind["EdgionPlugins"][0].1,
            EtcdDocument::Text(later_document.clone())
        );
        assert_eq!(snapshot.revision, 42);
    }
    let mut later = watcher.subscribe("EdgionPlugins").await;
    watcher.dispatch_init(&["EdgionPlugins"]).await.unwrap();
    assert!(matches!(later.recv().await.unwrap(), EtcdEvent::Init));
    match later.recv().await.unwrap() {
        EtcdEvent::InitApply { key, content } => {
            assert_eq!(key, later_key);
            assert_eq!(content, EtcdDocument::Text(later_document));
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
        by_kind: HashMap::from([("Secret", vec![(key.clone(), EtcdDocument::Text("document".into()))])]),
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

#[tokio::test]
async fn prime_preserves_invalid_document_presence_and_init_order() {
    let watcher = watcher_without_connection();
    let key = "/edgion/conf/HTTPRoute/default/invalid".to_owned();
    watcher.install_prime(
        &[
            (key.clone(), vec![0xff], 40),
            (
                "/edgion/conf/HTTPRoute/default/valid".into(),
                b"valid text".to_vec(),
                41,
            ),
            ("/other/conf/HTTPRoute/default/ignored".into(), vec![0xff], 41),
        ],
        42,
    );
    assert_eq!(watcher.known_keys.read()["HTTPRoute"].len(), 2);
    assert!(watcher.known_keys.read()["HTTPRoute"].contains(&key));
    let mut rx = watcher.subscribe("HTTPRoute").await;
    watcher.dispatch_init(&["HTTPRoute"]).await.unwrap();
    assert!(matches!(rx.recv().await.unwrap(), EtcdEvent::Init));
    assert!(
        matches!(rx.recv().await.unwrap(), EtcdEvent::InitApply { key: actual, content: EtcdDocument::InvalidUtf8 } if actual == key)
    );
    assert!(
        matches!(rx.recv().await.unwrap(), EtcdEvent::InitApply { content: EtcdDocument::Text(text), .. } if text == "valid text")
    );
    assert!(matches!(rx.recv().await.unwrap(), EtcdEvent::InitDone));
    assert!(rx.try_recv().is_err());
    assert_eq!(watcher.release_primed_documents().unwrap(), 42);
}

#[tokio::test]
async fn resync_distinguishes_invalid_presence_repair_and_actual_absence() {
    let watcher = watcher_without_connection();
    let key = "/edgion/conf/HTTPRoute/default/invalid".to_owned();
    let absent = "/edgion/conf/HTTPRoute/default/absent".to_owned();
    watcher.install_prime(
        &[(key.clone(), b"old".to_vec(), 1), (absent.clone(), b"old".to_vec(), 1)],
        1,
    );
    let mut rx = watcher.subscribe("HTTPRoute").await;
    watcher.dispatch_resync(&[(key.clone(), vec![0xff], 2)], 3).await;
    assert!(
        matches!(rx.recv().await.unwrap(), EtcdEvent::Apply { key: actual, content: EtcdDocument::InvalidUtf8 } if actual == key)
    );
    assert!(matches!(rx.recv().await.unwrap(), EtcdEvent::Delete { key: actual } if actual == absent));
    assert!(rx.try_recv().is_err());
    assert_eq!(watcher.known_keys.read()["HTTPRoute"], HashSet::from([key.clone()]));
    watcher
        .dispatch_resync(&[(key.clone(), b"repaired".to_vec(), 4)], 4)
        .await;
    assert!(
        matches!(rx.recv().await.unwrap(), EtcdEvent::Apply { content: EtcdDocument::Text(text), .. } if text == "repaired")
    );
    assert!(rx.try_recv().is_err());
    watcher.dispatch_resync(&[], 5).await;
    assert!(matches!(rx.recv().await.unwrap(), EtcdEvent::Delete { key: actual } if actual == key));
    assert!(watcher.known_keys.read().is_empty());
}

#[tokio::test]
async fn live_watch_delivers_invalid_put_and_preserves_revision_and_presence() {
    let watcher = watcher_without_connection();
    let key = "/edgion/conf/HTTPRoute/default/r1";
    let mut rx = watcher.subscribe("HTTPRoute").await;
    let senders = watcher.senders.read().await.clone();
    let mut revision = 10;
    watcher
        .dispatch_watch_event(&senders, key, &[0xff], EventType::Put, 11, &mut revision)
        .await;
    assert_eq!(revision, 11);
    assert!(matches!(
        rx.recv().await.unwrap(),
        EtcdEvent::Apply {
            content: EtcdDocument::InvalidUtf8,
            ..
        }
    ));
    assert!(watcher.known_keys.read()["HTTPRoute"].contains(key));
    watcher
        .dispatch_watch_event(&senders, key, b"repaired", EventType::Put, 12, &mut revision)
        .await;
    assert!(
        matches!(rx.recv().await.unwrap(), EtcdEvent::Apply { content: EtcdDocument::Text(text), .. } if text == "repaired")
    );
    watcher
        .dispatch_watch_event(&senders, key, &[], EventType::Delete, 13, &mut revision)
        .await;
    assert!(matches!(rx.recv().await.unwrap(), EtcdEvent::Delete { key: actual } if actual == key));
    assert!(!watcher.known_keys.read()["HTTPRoute"].contains(key));
    for ignored in [
        "/other/conf/HTTPRoute/default/r1",
        "/edgion/conf/Unknown/default/r1",
        "/edgion/conf/Secret/default/unsubscribed",
    ] {
        watcher
            .dispatch_watch_event(&senders, ignored, &[0xff], EventType::Put, 14, &mut revision)
            .await;
        assert_eq!(revision, 14);
        assert!(rx.try_recv().is_err());
    }
}
