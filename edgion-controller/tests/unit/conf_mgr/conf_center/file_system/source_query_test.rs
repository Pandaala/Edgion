use super::super::super::path_registry::{PathRegistry, ResourceKey};
use super::*;
use crate::conf_mgr::conf_center::traits::CenterApi;

fn write(root: &Path, namespace: &str, name: &str) -> PathBuf {
    let path = root.join(format!("ConfigMap_{namespace}_{name}.yaml"));
    std::fs::write(&path,format!("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  namespace: {namespace}\n  name: {name}\ndata:\n  original: source-value\n")).unwrap();
    path
}
fn options(token: Option<String>) -> Option<ListOptions> {
    Some(ListOptions {
        limit: 10,
        continue_token: token,
    })
}

#[tokio::test]
async fn pages_and_count_share_exact_scope_and_never_merge_status() {
    let root = tempfile::tempdir().unwrap();
    for i in (0..23).rev() {
        write(root.path(), "apps", &format!("resource-{i:02}"));
    }
    write(root.path(), "other", "outside");
    std::fs::write(
        root.path().join("ConfigMap_apps_resource-00.yaml.status"),
        "resolvedSecret: forbidden",
    )
    .unwrap();
    let storage = FileSystemStorage::new(root.path());
    assert_eq!(
        storage.count_by_kind("ConfigMap", Some("apps")).await.unwrap().total,
        23
    );
    assert_eq!(storage.count_by_kind("ConfigMap", None).await.unwrap().total, 24);
    let first = storage
        .get_list_by_kind_ns("ConfigMap", "apps", options(None))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 10);
    assert_eq!(first.items[0].name, "resource-00");
    assert!(!first.items[0].content.contains("resolvedSecret"));
    let second = storage
        .get_list_by_kind_ns("ConfigMap", "apps", options(first.continue_token.clone()))
        .await
        .unwrap();
    assert_eq!(second.items[0].name, "resource-10");
    assert_eq!(second.items.len(), 10);
    let repeated = storage
        .get_list_by_kind_ns("ConfigMap", "apps", options(first.continue_token))
        .await
        .unwrap();
    assert_eq!(repeated.items[0].content, second.items[0].content);
    let third = storage
        .get_list_by_kind_ns("ConfigMap", "apps", options(second.continue_token))
        .await
        .unwrap();
    assert_eq!(third.items.len(), 3);
    assert!(third.continue_token.is_none());
}

#[tokio::test]
async fn cursor_scope_page_size_and_storage_generation_are_bound() {
    let root = tempfile::tempdir().unwrap();
    for i in 0..11 {
        write(root.path(), "apps", &format!("r-{i:02}"));
    }
    let storage = FileSystemStorage::new(root.path());
    let token = storage
        .get_list_by_kind("ConfigMap", options(None))
        .await
        .unwrap()
        .continue_token;
    assert!(matches!(
        storage
            .get_list_by_kind_ns("ConfigMap", "apps", options(token.clone()))
            .await,
        Err(ConfWriterError::StalePagination(_))
    ));
    assert!(matches!(
        storage.get_list_by_kind("Secret", options(token.clone())).await,
        Err(ConfWriterError::StalePagination(_))
    ));
    assert!(matches!(
        storage
            .get_list_by_kind(
                "ConfigMap",
                Some(ListOptions {
                    limit: 20,
                    continue_token: token.clone()
                })
            )
            .await,
        Err(ConfWriterError::StalePagination(_))
    ));
    let replacement = FileSystemStorage::new(root.path());
    assert!(matches!(
        replacement.get_list_by_kind("ConfigMap", options(token)).await,
        Err(ConfWriterError::StalePagination(_))
    ));
    for limit in [0, 101] {
        assert!(storage
            .get_list_by_kind(
                "ConfigMap",
                Some(ListOptions {
                    limit,
                    continue_token: None
                })
            )
            .await
            .is_err());
    }
}

#[tokio::test]
async fn malformed_sources_fail_both_queries_and_registry_losers_are_excluded_first() {
    let root = tempfile::tempdir().unwrap();
    let winner = write(root.path(), "apps", "one");
    let nested = root.path().join("duplicate");
    std::fs::create_dir(&nested).unwrap();
    let loser = write(&nested, "apps", "one");
    let registry = Arc::new(PathRegistry::new());
    registry.record(ResourceKey::new("ConfigMap", "apps/one"), winner.clone());
    registry.record(ResourceKey::new("ConfigMap", "apps/one"), loser.clone());
    std::fs::write(&loser, "malformed: [").unwrap();
    let storage = FileSystemStorage::new(root.path()).with_registry(registry);
    assert_eq!(storage.count_by_kind("ConfigMap", None).await.unwrap().total, 1);
    assert_eq!(
        storage
            .get_list_by_kind("ConfigMap", options(None))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    std::fs::write(winner, "malformed: [").unwrap();
    assert!(storage.count_by_kind("ConfigMap", None).await.is_err());
    assert!(storage.get_list_by_kind("ConfigMap", options(None)).await.is_err());
}

#[tokio::test]
async fn unregistered_duplicates_and_oversized_files_do_not_produce_partial_success() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "apps", "one");
    let nested = root.path().join("duplicate");
    std::fs::create_dir(&nested).unwrap();
    let duplicate = write(&nested, "apps", "one");
    let storage = FileSystemStorage::new(root.path());
    assert!(storage.count_by_kind("ConfigMap", None).await.is_err());
    assert!(storage.get_list_by_kind("ConfigMap", options(None)).await.is_err());
    std::fs::remove_file(duplicate).unwrap();
    let huge = root.path().join("ConfigMap_apps_huge.yaml");
    std::fs::File::create(huge)
        .unwrap()
        .set_len((MAX_SOURCE_BYTES + 1) as u64)
        .unwrap();
    assert!(matches!(
        storage.count_by_kind("ConfigMap", None).await,
        Err(ConfWriterError::SourceBudget)
    ));
    assert!(matches!(
        storage.get_list_by_kind("ConfigMap", options(None)).await,
        Err(ConfWriterError::SourceBudget)
    ));
}

#[tokio::test]
async fn cancellation_does_not_release_a_running_blocking_scan_permit() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = semaphore.clone().acquire_owned().await.unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(blocking_scan(permit, move |cancelled| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        done_tx.send(cancelled.load(Ordering::Relaxed)).unwrap();
        Ok(())
    }));
    started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(semaphore.available_permits(), 0);
    release_tx.send(()).unwrap();
    assert!(done_rx.await.unwrap());
    let _permit = tokio::time::timeout(std::time::Duration::from_secs(1), semaphore.acquire())
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn selected_file_change_requires_refresh_and_expired_scan_reads_nothing() {
    let root = tempfile::tempdir().unwrap();
    let path = write(root.path(), "apps", "one");
    let content = std::fs::read(&path).unwrap();
    let candidate = Candidate {
        path: path.clone(),
        digest: Sha256::digest(&content).into(),
    };
    std::fs::write(&path, "changed").unwrap();
    let mut budget = ScanBudget {
        deadline: Instant::now() + SOURCE_TIMEOUT,
        cancelled: Arc::new(AtomicBool::new(false)),
        bytes: 0,
    };
    assert!(matches!(
        candidate.read_unchanged(&mut budget),
        Err(ConfWriterError::StalePagination(_))
    ));
    budget.deadline = Instant::now();
    budget.bytes = 0;
    assert!(matches!(budget.read(&path), Err(ConfWriterError::SourceTimeout)));
    assert_eq!(budget.bytes, 0);
}

#[tokio::test]
async fn cluster_scoped_source_pages_and_count_use_empty_namespace_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("GatewayClass__edge.yaml"),
        "apiVersion: gateway.networking.k8s.io/v1\nkind: GatewayClass\nmetadata:\n  name: edge\nspec: {}\n",
    )
    .unwrap();
    let storage = FileSystemStorage::new(root.path());
    assert_eq!(storage.count_by_kind("GatewayClass", None).await.unwrap().total, 1);
    let page = storage.get_list_by_kind("GatewayClass", options(None)).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].namespace, None);
    assert!(page.continue_token.is_none());
}

#[tokio::test]
async fn source_order_is_namespace_then_name_even_for_prefix_namespaces() {
    let root = tempfile::tempdir().unwrap();
    for ns in ["a-b", "a"] {
        for name in ["z", "a"] {
            write(root.path(), ns, name);
        }
    }
    let storage = FileSystemStorage::new(root.path());
    let mut token = None;
    let mut identities = Vec::new();
    loop {
        let page = storage
            .get_list_by_kind(
                "ConfigMap",
                Some(ListOptions {
                    limit: 1,
                    continue_token: token,
                }),
            )
            .await
            .unwrap();
        identities.extend(page.items.into_iter().map(|item| (item.namespace.unwrap(), item.name)));
        token = page.continue_token;
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        identities,
        vec![
            ("a".into(), "a".into()),
            ("a".into(), "z".into()),
            ("a-b".into(), "a".into()),
            ("a-b".into(), "z".into())
        ]
    );
}

#[tokio::test]
async fn exact_source_reads_are_bounded_fresh_and_use_registry_winner() {
    let root = tempfile::tempdir().unwrap();
    let path = write(root.path(), "apps", "one");
    let registry = Arc::new(PathRegistry::new());
    registry.record(ResourceKey::new("ConfigMap", "apps/one"), path.clone());
    let storage = FileSystemStorage::new(root.path()).with_registry(registry);
    let first = storage.get_source_one("ConfigMap", Some("apps"), "one").await.unwrap();
    assert!(first.contains("source-value"));
    std::fs::write(&path, first.replace("source-value", "new-value")).unwrap();
    assert!(storage
        .get_source_one("ConfigMap", Some("apps"), "one")
        .await
        .unwrap()
        .contains("new-value"));
    std::fs::write(&path, vec![b'x'; MAX_SOURCE_BYTES + 1]).unwrap();
    assert!(matches!(
        storage.get_source_one("ConfigMap", Some("apps"), "one").await,
        Err(ConfWriterError::SourceBudget)
    ));
    assert!(matches!(
        storage.get_source_one("ConfigMap", Some("apps"), "missing").await,
        Err(ConfWriterError::NotFound(_))
    ));
    assert!(matches!(
        storage.get_source_one("ConfigMap", Some("../apps"), "one").await,
        Err(ConfWriterError::ValidationError(_))
    ));
}

#[tokio::test]
async fn source_cas_serializes_competing_writers_and_rejects_same_version_edits() {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemStorage::new(root.path());
    let original =
        "kind: ConfigMap\nmetadata: {namespace: apps, name: one, resourceVersion: '7'}\ndata: {value: original}\n";
    storage
        .set_one("ConfigMap", Some("apps"), "one", original.into())
        .await
        .unwrap();
    let expected = SourcePrecondition::from_value(&yaml_serde::from_str(original).unwrap()).unwrap();
    let (a, b) = tokio::join!(
        storage.update_one_if_source(
            "ConfigMap",
            Some("apps"),
            "one",
            original.replace("original", "first"),
            &expected
        ),
        storage.update_one_if_source(
            "ConfigMap",
            Some("apps"),
            "one",
            original.replace("original", "second"),
            &expected
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(a, Err(ConfWriterError::Conflict(_))) || matches!(b, Err(ConfWriterError::Conflict(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_source_commit_keeps_lock_and_skips_unstarted_rename() {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemStorage::new(root.path());
    let path = root.path().join("object.yaml");
    std::fs::write(&path, "original").unwrap();
    let lock = storage.update_lock("ConfigMap", Some("apps"), "one");
    let guard = lock.lock().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = permits.clone().acquire_owned().await.unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let target = path.clone();
    let owner = tokio::spawn(async move {
        blocking_scan(permit, move |cancelled| {
            let _guard = guard;
            super::super::write_existing_atomic_sync(&target, "old", || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                if cancelled.load(Ordering::Relaxed) {
                    Err(ConfWriterError::SourceTimeout)
                } else {
                    Ok(())
                }
            })
        })
        .await
    });
    started_rx.await.unwrap();
    owner.abort();
    let _ = owner.await;
    assert!(lock.try_lock().is_err());
    assert_eq!(permits.available_permits(), 0);
    release_tx.send(()).unwrap();
    let _guard = lock.lock().await;
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    let returned = permits.acquire().await.unwrap();
    drop(returned);
    assert_eq!(permits.available_permits(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_started_replace_and_unlink_hold_lock_until_actual_completion() {
    for delete in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let storage = FileSystemStorage::new(root.path());
        let path = root.path().join("object.yaml");
        std::fs::write(&path, "original").unwrap();
        let lock = storage.update_lock("ConfigMap", Some("apps"), "one");
        let guard = lock.lock().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let target = path.clone();
        let owner = tokio::spawn(async move {
            super::super::blocking_mutation(guard, move || {
                let started = || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                };
                if delete {
                    super::super::delete_existing_sync(&target, None, started)
                } else {
                    super::super::write_existing_atomic_sync(&target, "old", || {
                        started();
                        Ok(())
                    })
                }
            })
            .await
        });
        started_rx.await.unwrap();
        owner.abort();
        let _ = owner.await;
        assert!(lock.try_lock().is_err(), "actual filesystem work still owns the lock");
        let target = path.clone();
        let subsequent = tokio::spawn(async move {
            let guard = lock.lock().await;
            super::super::blocking_mutation(guard, move || {
                super::super::write_existing_atomic_sync(&target, "new", || Ok(()))
            })
            .await
            .unwrap();
        });
        release_tx.send(()).unwrap();
        subsequent.await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new\n");
    }
}
