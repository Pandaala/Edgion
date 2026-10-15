use super::*;
use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

#[tokio::test]
async fn init_documents_release_per_subscribed_phase() {
    let dir = tempfile::tempdir().unwrap();
    let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), Arc::new(PathRegistry::new()), None);
    let first_path = dir.path().join("Secret_default_first.yaml");
    let later_path = dir.path().join("EdgionPlugins_default_later.yaml");
    let first_document = "first document".to_owned();
    let later_document = "later document".to_owned();
    *watcher.primed.lock() = Some(HashMap::from([
        ("Secret", vec![(first_path.clone(), first_document.clone())]),
        ("EdgionPlugins", vec![(later_path.clone(), later_document.clone())]),
    ]));
    let mut first = watcher.subscribe("Secret").await;
    let mut empty = watcher.subscribe("Gateway").await;
    watcher
        .dispatch_init(&["Secret", "EdgionPlugins", "Gateway"])
        .await
        .unwrap();
    assert!(matches!(first.recv().await.unwrap(), FileSystemEvent::Init));
    match first.recv().await.unwrap() {
        FileSystemEvent::InitApply { path, content } => {
            assert_eq!(path, first_path);
            assert_eq!(content, first_document);
        }
        event => panic!("unexpected event: {event:?}"),
    }
    assert!(matches!(first.recv().await.unwrap(), FileSystemEvent::InitDone));
    assert!(matches!(empty.recv().await.unwrap(), FileSystemEvent::Init));
    assert!(matches!(empty.recv().await.unwrap(), FileSystemEvent::InitDone));
    {
        let guard = watcher.primed.lock();
        let remaining = guard.as_ref().unwrap();
        assert!(!remaining.contains_key("Secret"));
        assert_eq!(remaining["EdgionPlugins"][0].1, later_document);
    }
    let mut later = watcher.subscribe("EdgionPlugins").await;
    watcher.dispatch_init(&["EdgionPlugins"]).await.unwrap();
    assert!(matches!(later.recv().await.unwrap(), FileSystemEvent::Init));
    match later.recv().await.unwrap() {
        FileSystemEvent::InitApply { path, content } => {
            assert_eq!(path, later_path);
            assert_eq!(content, later_document);
        }
        event => panic!("unexpected event: {event:?}"),
    }
    assert!(matches!(later.recv().await.unwrap(), FileSystemEvent::InitDone));
    assert!(watcher.primed.lock().as_ref().unwrap().is_empty());
}

#[tokio::test]
async fn runtime_releases_leftover_documents_without_requiring_prime() {
    let dir = tempfile::tempdir().unwrap();
    for primed in [
        None,
        Some(HashMap::from([(
            "Secret",
            vec![(PathBuf::from("unused"), "document".into())],
        )])),
    ] {
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), Arc::new(PathRegistry::new()), None);
        *watcher.primed.lock() = primed;
        let shutdown = crate::conf_mgr::sync_runtime::ShutdownHandle::new();
        shutdown.shutdown();
        tokio::time::timeout(Duration::from_secs(2), watcher.run_runtime(shutdown.signal()))
            .await
            .unwrap()
            .unwrap();
        assert!(watcher.primed.lock().is_none());
    }
}

#[test]
fn poll_interval_clamps_at_min_for_empty_or_small_conf() {
    assert_eq!(poll_interval_for(0), Duration::from_secs(1));
    assert_eq!(poll_interval_for(1), Duration::from_secs(1));
    assert_eq!(poll_interval_for(2_000), Duration::from_secs(1));
}

#[test]
fn poll_interval_steps_every_2000_files() {
    assert_eq!(poll_interval_for(2_001), Duration::from_secs(2));
    assert_eq!(poll_interval_for(4_000), Duration::from_secs(2));
    assert_eq!(poll_interval_for(4_001), Duration::from_secs(3));
    assert_eq!(poll_interval_for(6_000), Duration::from_secs(3));
    assert_eq!(poll_interval_for(6_001), Duration::from_secs(4));
    assert_eq!(poll_interval_for(8_000), Duration::from_secs(4));
}

#[test]
fn poll_interval_clamps_at_max_for_large_conf() {
    assert_eq!(poll_interval_for(8_001), Duration::from_secs(5));
    assert_eq!(poll_interval_for(10_000), Duration::from_secs(5));
    assert_eq!(poll_interval_for(50_000), Duration::from_secs(5));
    assert_eq!(poll_interval_for(usize::MAX), Duration::from_secs(5));
}

#[test]
fn remove_file_event_triggers_delete() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Remove(RemoveKind::File), false),
        DispatchAction::Delete
    );
}

#[test]
fn remove_any_event_triggers_delete() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Remove(RemoveKind::Any), false),
        DispatchAction::Delete
    );
}

#[test]
fn remove_event_triggers_delete_even_when_path_exists() {
    // Conservative: trust the kernel Remove event, not the existence check.
    assert_eq!(
        infer_dispatch_action(&EventKind::Remove(RemoveKind::File), true),
        DispatchAction::Delete
    );
}

#[test]
fn rename_from_event_triggers_delete() {
    // Modify(Name(From)) = file was renamed/moved away = effectively deleted.
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::From)), false),
        DispatchAction::Delete
    );
}

#[test]
fn rename_from_event_triggers_delete_even_when_path_exists() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::From)), true),
        DispatchAction::Delete
    );
}

#[test]
fn create_event_with_existing_file_triggers_apply() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Create(CreateKind::File), true),
        DispatchAction::Apply
    );
}

#[test]
fn rename_to_event_triggers_apply() {
    // Modify(Name(To)) = file was renamed/moved into watched dir.
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::To)), true),
        DispatchAction::Apply
    );
}

#[test]
fn modify_data_event_with_existing_file_triggers_apply() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Data(DataChange::Content)), true),
        DispatchAction::Apply
    );
}

#[test]
fn modify_data_event_with_missing_file_triggers_recheck() {
    // Ambiguous-kind defense-in-depth: a Modify whose path no longer exists could
    // be an in-flight rename (file briefly absent between unlink and link).
    // PollWatcher never reports this kind, but the classifier must still answer.
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Data(DataChange::Content)), false),
        DispatchAction::Recheck
    );
}

#[test]
fn any_event_with_existing_file_triggers_apply() {
    assert_eq!(infer_dispatch_action(&EventKind::Any, true), DispatchAction::Apply);
}

#[test]
fn any_event_with_missing_file_triggers_recheck() {
    assert_eq!(infer_dispatch_action(&EventKind::Any, false), DispatchAction::Recheck);
}

// Modify(Name(To)) — the destination half of an atomic rename. Whether the
// sequence reaches us this way (notify backends differ in event granularity)
// or as a single Modify, file existing → Apply.
#[test]
fn rename_to_with_existing_file_triggers_apply() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::To)), true),
        DispatchAction::Apply
    );
}

// Plain Create on an existing file → Apply.
#[test]
fn create_with_existing_file_triggers_apply() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Create(CreateKind::Any), true),
        DispatchAction::Apply
    );
}

// Rename(Both) is uncertain (two paths in one event, per-path semantic unclear).
#[test]
fn rename_both_with_existing_file_triggers_apply() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::Both)), true),
        DispatchAction::Apply
    );
}

#[test]
fn rename_both_with_missing_file_triggers_recheck() {
    assert_eq!(
        infer_dispatch_action(&EventKind::Modify(ModifyKind::Name(RenameMode::Both)), false),
        DispatchAction::Recheck
    );
}

#[tokio::test]
async fn init_phase_lex_winner_for_collision() {
    use crate::conf_mgr::conf_center::file_system::path_registry::{PathRegistry, ResourceKey};
    use std::sync::Arc;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let yaml =
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\n";
    std::fs::write(a.join("HTTPRoute_default_r1.yaml"), yaml).unwrap();
    std::fs::write(b.join("HTTPRoute_default_r1.yaml"), yaml).unwrap();

    let registry = Arc::new(PathRegistry::new());
    let watcher = FileSystemWatcher::new(root, registry.clone(), None);
    watcher.init_phase().await.unwrap();

    let key = ResourceKey::new("HTTPRoute".to_string(), "default/r1".to_string());
    let winner = registry.winner_of(&key).expect("winner exists");
    assert!(
        winner.to_string_lossy().contains("/a/"),
        "expected lex-smallest path to win, got {:?}",
        winner
    );
    let conflicts = registry.snapshot_conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].losers.len(), 1);
}

#[tokio::test]
async fn dispatch_init_is_phased() {
    use super::super::event::FileSystemEvent;
    let dir = tempfile::tempdir().unwrap();
    // One foundation file (Gateway) and one dependent file (HTTPRoute).
    std::fs::write(
        dir.path().join("Gateway_default_gw.yaml"),
        "apiVersion: gateway.networking.k8s.io/v1\nkind: Gateway\nmetadata:\n  name: gw\n  namespace: default\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("HTTPRoute_default_rt.yaml"),
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: rt\n  namespace: default\n",
    )
    .unwrap();

    let registry = std::sync::Arc::new(PathRegistry::new());
    let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), registry, None);
    let mut gw_rx = watcher.subscribe("Gateway").await;
    let mut rt_rx = watcher.subscribe("HTTPRoute").await;

    watcher.prime().await.unwrap();
    watcher.dispatch_init(&["Gateway"]).await.unwrap();

    // Gateway got Init -> InitApply -> InitDone.
    assert!(matches!(gw_rx.recv().await.unwrap(), FileSystemEvent::Init));
    assert!(matches!(gw_rx.recv().await.unwrap(), FileSystemEvent::InitApply { .. }));
    assert!(matches!(gw_rx.recv().await.unwrap(), FileSystemEvent::InitDone));
    // HTTPRoute has NOT been dispatched yet.
    assert!(
        rt_rx.try_recv().is_err(),
        "Phase-2 kind must not receive before its dispatch"
    );

    watcher.dispatch_init(&["HTTPRoute"]).await.unwrap();
    assert!(matches!(rt_rx.recv().await.unwrap(), FileSystemEvent::Init));
    assert!(matches!(rt_rx.recv().await.unwrap(), FileSystemEvent::InitApply { .. }));
    assert!(matches!(rt_rx.recv().await.unwrap(), FileSystemEvent::InitDone));
}

#[tokio::test]
async fn second_watcher_generation_replays_existing_resources() {
    use crate::conf_mgr::conf_center::file_system::event::FileSystemEvent;
    use crate::conf_mgr::conf_center::file_system::path_registry::PathRegistry;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn init_apply_count(watcher: &FileSystemWatcher) -> usize {
        let mut rx = watcher.subscribe("HTTPRoute").await;
        watcher.init_phase().await.unwrap();
        let mut count = 0;
        loop {
            match rx.recv().await.unwrap() {
                FileSystemEvent::InitApply { .. } => count += 1,
                FileSystemEvent::InitDone => return count,
                _ => {}
            }
        }
    }

    let dir = TempDir::new().unwrap();
    let yaml =
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\n";
    std::fs::write(dir.path().join("HTTPRoute_default_r1.yaml"), yaml).unwrap();
    let registry = Arc::new(PathRegistry::new());

    let first = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
    assert_eq!(init_apply_count(&first).await, 1);

    let second = FileSystemWatcher::new(dir.path().to_path_buf(), registry, None);
    assert_eq!(init_apply_count(&second).await, 1);
}

// ARCH-047: exercise the real scan and channel boundary, including misleading
// nested kinds. A successful envelope lookup alone would not prove dispatch.
#[tokio::test]
async fn structured_root_kind_controls_init_channel() {
    let documents = [
        "apiVersion: gateway.networking.k8s.io/v1\n'kind': 'HTTPRoute'\nmetadata: {name: route, namespace: default}\nspec: {}\n",
        r#"{"spec":{"rules":[{"backendRefs":[{"name":"backend","kind":"Service","port":80}]}]},"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"route","namespace":"default"}}"#,
        "kind: HTTPRoute\napiVersion: gateway.networking.k8s.io/v1\nmetadata: {name: route, namespace: default}\nspec: {rules: [{backendRefs: [{name: backend, kind: Service, port: 80}]}]}\n",
        "metadata: {name: route, namespace: default}\napiVersion: gateway.networking.k8s.io/v1\n\"kind\": HTTPRoute\nspec: {}\n",
    ];
    for document in documents {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HTTPRoute_default_route.yaml");
        std::fs::write(&path, document).unwrap();
        let registry = Arc::new(PathRegistry::new());
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
        let mut routes = watcher.subscribe("HTTPRoute").await;
        let mut services = watcher.subscribe("Service").await;
        watcher.prime().await.unwrap();
        assert!(routes.try_recv().is_err());
        assert!(services.try_recv().is_err());
        assert_eq!(
            registry.winner_of(&ResourceKey::new("HTTPRoute", "default/route")),
            Some(path.clone())
        );
        watcher.dispatch_init(&["HTTPRoute", "Service"]).await.unwrap();
        assert!(matches!(routes.try_recv().unwrap(), FileSystemEvent::Init));
        match routes.try_recv().unwrap() {
            FileSystemEvent::InitApply { path: actual, content } => {
                assert_eq!(actual, path);
                assert_eq!(content, document);
            }
            event => panic!("unexpected route event: {event:?}"),
        }
        assert!(matches!(routes.try_recv().unwrap(), FileSystemEvent::InitDone));
        assert!(routes.try_recv().is_err());
        assert!(matches!(services.try_recv().unwrap(), FileSystemEvent::Init));
        assert!(matches!(services.try_recv().unwrap(), FileSystemEvent::InitDone));
        assert!(services.try_recv().is_err());
        assert!(watcher.primed.lock().as_ref().unwrap().is_empty());
    }
}

#[tokio::test]
async fn invalid_init_envelopes_do_not_register_or_dispatch() {
    for document in [
        "kind: HTTPRoute\nmetadata: [",
        "kind: UnknownResource\nmetadata: {}",
        "kind: 42\nmetadata: {}",
        "kind: null\nmetadata: {}",
        "kind: [HTTPRoute]\nmetadata: {}",
        "metadata: {}\nspec: {kind: HTTPRoute}",
        "kind: HTTPRoute",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HTTPRoute_default_route.yaml");
        std::fs::write(&path, document).unwrap();
        let registry = Arc::new(PathRegistry::new());
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
        let mut routes = watcher.subscribe("HTTPRoute").await;
        watcher.prime().await.unwrap();
        assert!(registry
            .winner_of(&ResourceKey::new("HTTPRoute", "default/route"))
            .is_none());
        watcher.dispatch_init(&["HTTPRoute"]).await.unwrap();
        assert!(matches!(routes.try_recv().unwrap(), FileSystemEvent::Init));
        assert!(matches!(routes.try_recv().unwrap(), FileSystemEvent::InitDone));
        assert!(routes.try_recv().is_err());
    }
}

#[tokio::test]
async fn repaired_structured_envelope_reloads_with_filename_collision_identity() {
    let dir = tempfile::tempdir().unwrap();
    let first_dir = dir.path().join("a");
    let second_dir = dir.path().join("b");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();
    let winner = first_dir.join("HTTPRoute_default_route.yaml");
    let loser = second_dir.join("HTTPRoute_default_route.yaml");
    std::fs::write(&winner, "kind: HTTPRoute\nmetadata: [").unwrap();
    let registry = Arc::new(PathRegistry::new());
    let key = ResourceKey::new("HTTPRoute", "default/route");
    let first = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
    first.prime().await.unwrap();
    assert!(registry.winner_of(&key).is_none());

    // Scan identity stays filename-owned. Typed processing, not prime, rejects
    // the deliberate metadata mismatch; the second file remains a conflict.
    let repaired = "apiVersion: gateway.networking.k8s.io/v1\n'kind': HTTPRoute\nmetadata: {name: different, namespace: other}\nspec: {}";
    std::fs::write(&winner, repaired).unwrap();
    std::fs::write(&loser, repaired).unwrap();
    for _ in 0..2 {
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
        let mut routes = watcher.subscribe("HTTPRoute").await;
        watcher.prime().await.unwrap();
        assert_eq!(registry.winner_of(&key), Some(winner.clone()));
        assert!(registry
            .winner_of(&ResourceKey::new("HTTPRoute", "other/different"))
            .is_none());
        let conflicts = registry.snapshot_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].losers, vec![loser.clone()]);
        watcher.dispatch_init(&["HTTPRoute"]).await.unwrap();
        assert!(matches!(routes.try_recv().unwrap(), FileSystemEvent::Init));
        match routes.try_recv().unwrap() {
            FileSystemEvent::InitApply { path, content } => {
                assert_eq!(path, winner);
                assert_eq!(content, repaired);
            }
            event => panic!("unexpected route event: {event:?}"),
        }
        assert!(matches!(routes.try_recv().unwrap(), FileSystemEvent::InitDone));
        assert!(routes.try_recv().is_err());
        assert!(watcher.primed.lock().as_ref().unwrap().is_empty());
    }
}

#[tokio::test]
async fn initial_delivery_backpressures_without_losing_documents() {
    let dir = tempfile::tempdir().unwrap();
    let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), Arc::new(PathRegistry::new()), None);
    let count = KIND_EVENT_CAPACITY + 17;
    let documents: Vec<_> = (0..count)
        .map(|i| {
            (
                dir.path().join(format!("HTTPRoute_default_route-{i}.yaml")),
                format!("document-{i}"),
            )
        })
        .collect();
    *watcher.primed.lock() = Some(HashMap::from([("HTTPRoute", documents.clone())]));
    let mut rx = watcher.subscribe("HTTPRoute").await;
    let sender = watcher.senders.read().await["HTTPRoute"].clone();
    let mut dispatch = Box::pin(watcher.dispatch_init(&["HTTPRoute"]));
    tokio::time::timeout(Duration::from_secs(5), async {
        while sender.capacity() != 0 {
            assert!(futures::poll!(&mut dispatch).is_pending());
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(rx.len(), KIND_EVENT_CAPACITY);
    assert!(futures::poll!(&mut dispatch).is_pending());
    assert!(watcher.senders.try_write().is_ok());
    assert!(watcher.primed.try_lock().unwrap().as_ref().unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(5), async {
        let receive = async {
            assert!(matches!(rx.recv().await, Some(FileSystemEvent::Init)));
            for (expected_path, expected_content) in documents {
                match rx.recv().await.unwrap() {
                    FileSystemEvent::InitApply { path, content } => {
                        assert_eq!(path, expected_path);
                        assert_eq!(content, expected_content);
                    }
                    event => panic!("unexpected event: {event:?}"),
                }
            }
            assert!(matches!(rx.recv().await, Some(FileSystemEvent::InitDone)));
            assert!(rx.try_recv().is_err());
        };
        let (result, ()) = tokio::join!(&mut dispatch, receive);
        result.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn initial_receiver_closure_fails_every_delivery_stage_without_payload() {
    for stage in ["init", "document", "done"] {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), Arc::new(PathRegistry::new()), None);
        let documents = vec![(
            dir.path().join("Secret_default_test.yaml"),
            "private-secret-sentinel".to_owned(),
        )];
        *watcher.primed.lock() = Some(HashMap::from([("Secret", documents)]));
        let capacity = if stage == "done" { 2 } else { 1 };
        let (tx, rx) = mpsc::channel(capacity);
        watcher.senders.write().await.insert("Secret", tx.clone());
        let mut dispatch = Box::pin(watcher.dispatch_init(&["Secret"]));
        if stage != "init" {
            tokio::time::timeout(Duration::from_secs(2), async {
                while tx.capacity() != 0 {
                    assert!(futures::poll!(&mut dispatch).is_pending());
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(rx.len(), capacity);
            assert!(futures::poll!(&mut dispatch).is_pending());
        }
        drop(rx);
        let error = tokio::time::timeout(Duration::from_secs(2), dispatch)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "FS initialization receiver closed for kind Secret");
        assert!(!format!("{error:?}").contains("private-secret-sentinel"));
    }
}

#[tokio::test]
async fn duplicate_subscription_preserves_original_receiver() {
    use futures::FutureExt;
    use std::panic::AssertUnwindSafe;
    let dir = tempfile::tempdir().unwrap();
    let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), Arc::new(PathRegistry::new()), None);
    let mut rx = watcher.subscribe("Secret").await;
    assert!(AssertUnwindSafe(watcher.subscribe("Secret"))
        .catch_unwind()
        .await
        .is_err());
    watcher.senders.read().await["Secret"]
        .send(FileSystemEvent::Init)
        .await
        .unwrap();
    assert!(matches!(rx.recv().await, Some(FileSystemEvent::Init)));
}

#[tokio::test]
async fn runtime_full_channel_shutdown_abandons_mutated_generation() {
    use crate::conf_mgr::sync_runtime::ShutdownHandle;
    for action in ["apply", "delete", "promote"] {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(PathRegistry::new());
        let watcher = FileSystemWatcher::new(dir.path().to_path_buf(), registry.clone(), None);
        let path = dir.path().join("Secret_default_test.yaml");
        let promoted = dir.path().join("nested/Secret_default_test.yaml");
        let key = ResourceKey::new("Secret", "default/test");
        std::fs::write(&path, "private-secret-sentinel").unwrap();
        if action != "apply" {
            registry.record(key.clone(), path.clone());
        }
        if action == "promote" {
            std::fs::create_dir_all(promoted.parent().unwrap()).unwrap();
            std::fs::write(&promoted, "private-secret-sentinel").unwrap();
            registry.record(key.clone(), promoted.clone());
        }
        let rx = watcher.subscribe("Secret").await;
        let tx = watcher.senders.read().await["Secret"].clone();
        for _ in 0..KIND_EVENT_CAPACITY {
            tx.try_send(FileSystemEvent::Init).unwrap();
        }
        assert_eq!(tx.capacity(), 0);
        let shutdown = ShutdownHandle::new();
        let mut signal = shutdown.signal();
        let kind = if action == "apply" {
            EventKind::Create(CreateKind::File)
        } else {
            EventKind::Remove(RemoveKind::File)
        };
        let expected = match action {
            "apply" => Some(path.clone()),
            "promote" => Some(promoted),
            _ => None,
        };
        let mut dispatch = Box::pin(watcher.dispatch_until_shutdown(&path, kind, &mut signal));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                assert!(futures::poll!(&mut dispatch).is_pending());
                if registry.winner_of(&key) == expected {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(rx.len(), KIND_EVENT_CAPACITY);
        shutdown.shutdown();
        assert!(!tokio::time::timeout(Duration::from_secs(2), dispatch)
            .await
            .unwrap()
            .unwrap());
        assert_eq!(registry.winner_of(&key), expected);
        assert_eq!(rx.len(), KIND_EVENT_CAPACITY);
        assert!(watcher.senders.try_write().is_ok());
    }
}
