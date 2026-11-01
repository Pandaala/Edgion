use super::*;
use http::{Request, Response, StatusCode};
use kube::client::Body;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::service_fn;

fn response(status: StatusCode, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(value.to_string().into_bytes()))
        .unwrap()
}
fn row(ns: &str, name: &str) -> Value {
    json!({"metadata":{"namespace":ns,"name":name},"data":{"unknownFutureField":{"keep":true}}})
}
fn storage(
    namespaces: &[&str],
    f: impl Fn(Request<Body>) -> Response<Body> + Send + Sync + 'static,
) -> KubernetesStorage {
    let service = service_fn(move |request| {
        let reply = f(request);
        async move { Ok::<_, Infallible>(reply) }
    });
    let mut storage =
        KubernetesStorage::new_unconnected_for_test().with_client_for_test(Client::new(service, "default"));
    storage.allowed_namespaces = namespaces.iter().map(|s| s.to_string()).collect();
    storage
}
fn options(limit: u32, token: Option<String>) -> ListOptions {
    ListOptions {
        limit,
        continue_token: token,
    }
}

#[tokio::test]
async fn native_page_preserves_missing_typemeta_and_unknown_fields_and_binds_cursor() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let storage = storage(&["a", "b"], move |request| {
        captured.fetch_add(1, Ordering::Relaxed);
        assert!(request.uri().query().unwrap().contains("limit=1"));
        let ns = if request.uri().path().contains("/a/") {
            "a"
        } else {
            assert!(request.uri().path().contains("/b/"));
            "b"
        };
        response(
            StatusCode::OK,
            json!({"metadata":{"resourceVersion":"7"},"items":[row(ns,"one")]}),
        )
    });
    let first = storage.source_page("ConfigMap", None, options(1, None)).await.unwrap();
    assert_eq!(first.items[0].namespace.as_deref(), Some("a"));
    let value = query::document(&first.items[0]).unwrap();
    assert!(value.get("kind").is_none());
    assert_eq!(value["data"]["unknownFutureField"]["keep"], true);
    let token = first.continue_token.unwrap();
    for (kind, ns, limit) in [("Secret", None, 1), ("ConfigMap", Some("a"), 1), ("ConfigMap", None, 2)] {
        assert!(matches!(
            storage.source_page(kind, ns, options(limit, Some(token.clone()))).await,
            Err(ConfWriterError::StalePagination(_))
        ));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let second = storage
        .source_page("ConfigMap", None, options(1, Some(token)))
        .await
        .unwrap();
    assert_eq!(second.items[0].namespace.as_deref(), Some("b"));
    assert!(second.continue_token.is_none());
    assert!(matches!(
        storage
            .source_page("ConfigMap", Some("forbidden"), options(1, None))
            .await,
        Err(ConfWriterError::PermissionDenied(_))
    ));
    assert!(matches!(
        storage.source_count("ConfigMap", Some("forbidden")).await,
        Err(ConfWriterError::PermissionDenied(_))
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn namespace_hop_budget_returns_a_continuation_for_empty_pages() {
    let names: Vec<_> = (0..17).map(|i| format!("n{i:02}")).collect();
    let refs: Vec<_> = names.iter().map(String::as_str).collect();
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let storage = storage(&refs, move |_| {
        captured.fetch_add(1, Ordering::Relaxed);
        response(StatusCode::OK, json!({"metadata":{"resourceVersion":"7"},"items":[]}))
    });
    let first = storage.source_page("ConfigMap", None, options(10, None)).await.unwrap();
    assert!(first.items.is_empty());
    assert_eq!(calls.load(Ordering::Relaxed), 16);
    let second = storage
        .source_page("ConfigMap", None, options(10, first.continue_token))
        .await
        .unwrap();
    assert!(second.items.is_empty());
    assert!(second.continue_token.is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 17);
}

#[tokio::test]
async fn counts_use_metadata_only_fast_path_and_observed_scan_across_namespaces() {
    let storage = storage(&["a", "b"], move |request| {
        assert!(request.headers()["accept"]
            .to_str()
            .unwrap()
            .contains("PartialObjectMetadataList"));
        assert!(request.uri().query().unwrap().contains("limit=1"));
        let ns = if request.uri().path().contains("/a/") { "a" } else { "b" };
        response(
            StatusCode::OK,
            json!({"metadata":{"resourceVersion":"7","continue":"next","remainingItemCount":if ns=="a" {1}else{2}},"items":[row(ns,"one")]}),
        )
    });
    let all = storage.source_count("ConfigMap", None).await.unwrap();
    assert_eq!(all.total, 5);
    assert!(matches!(all.consistency, CountConsistency::ObservedScan));
    let single = storage.source_count("ConfigMap", Some("a")).await.unwrap();
    assert_eq!(single.total, 2);
    assert!(matches!(single.consistency, CountConsistency::SourceSnapshot));
}

#[tokio::test]
async fn count_fallback_follows_native_cursor_at_one_revision() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let storage = storage(&[], move |request| {
        let n = captured.fetch_add(1, Ordering::Relaxed);
        assert!(request.headers()["accept"]
            .to_str()
            .unwrap()
            .contains("PartialObjectMetadataList"));
        let query = request.uri().query().unwrap();
        if n == 0 {
            assert!(query.contains("limit=1"));
        } else {
            assert!(query.contains("limit=100"));
            assert!(query.contains(&format!("continue=p{n}")));
        }
        response(
            StatusCode::OK,
            json!({"metadata":{"resourceVersion":"7","continue":if n<2 {format!("p{}",n+1)}else{String::new()}},"items":if n<2 {vec![row("a",&format!("r{n}"))]}else{vec![]}}),
        )
    });
    assert_eq!(storage.source_count("ConfigMap", None).await.unwrap().total, 2);
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn count_discards_drifted_snapshot_and_retries_only_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let storage = storage(&[], move |_| {
        let n = captured.fetch_add(1, Ordering::Relaxed);
        response(
            StatusCode::OK,
            json!({"metadata":{"resourceVersion":format!("{}",7+n),"continue":if n==0{"next"}else{""}},"items":[row("a","one")]}),
        )
    });
    assert_eq!(storage.source_count("ConfigMap", None).await.unwrap().total, 1);
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let storage = storage_for_expiration(captured);
    assert!(matches!(
        storage.source_count("ConfigMap", None).await,
        Err(ConfWriterError::StalePagination(_))
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}
fn storage_for_expiration(calls: Arc<AtomicUsize>) -> KubernetesStorage {
    storage(&[], move |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        response(StatusCode::GONE, json!({"secret":"must not be logged"}))
    })
}

#[tokio::test]
async fn malformed_count_and_late_namespace_failure_never_return_partial_total() {
    for remaining in [-1] {
        let storage = storage(&[], move |_| {
            response(
                StatusCode::OK,
                json!({"metadata":{"resourceVersion":"7","remainingItemCount":remaining},"items":[]}),
            )
        });
        assert!(storage.source_count("ConfigMap", None).await.is_err());
    }
    let storage = storage(&["a", "b"], move |request| {
        if request.uri().path().contains("/b/") {
            return response(StatusCode::FORBIDDEN, json!({}));
        }
        response(
            StatusCode::OK,
            json!({"metadata":{"resourceVersion":"7","remainingItemCount":50},"items":[]}),
        )
    });
    assert!(matches!(
        storage.source_count("ConfigMap", None).await,
        Err(ConfWriterError::PermissionDenied(_))
    ));
}

#[tokio::test]
async fn malformed_or_over_limit_native_pages_fail_without_truncation() {
    for items in [
        vec![row("a", "one"), row("a", "two")],
        vec![row("wrong", "one")],
        vec![json!({"metadata":{"namespace":"a"}})],
    ] {
        let storage = storage(&["a"], move |_| {
            response(
                StatusCode::OK,
                json!({"metadata":{"resourceVersion":"7"},"items":items}),
            )
        });
        assert!(storage
            .source_page("ConfigMap", Some("a"), options(1, None))
            .await
            .is_err());
    }
    let storage = storage(&[], move |_| {
        Response::new(Body::from(vec![b'x'; MAX_SOURCE_BYTES + 1]))
    });
    assert!(matches!(
        storage.source_page("ConfigMap", None, options(10, None)).await,
        Err(ConfWriterError::SourceBudget)
    ));
}

#[tokio::test(start_paused = true)]
async fn count_retry_retains_the_original_deadline() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let service = service_fn(move |_request| {
        let n = captured.fetch_add(1, Ordering::Relaxed);
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            Ok::<_, Infallible>(if n == 0 {
                response(StatusCode::GONE, json!({}))
            } else {
                response(StatusCode::OK, json!({"metadata":{"resourceVersion":"7"},"items":[]}))
            })
        }
    });
    let storage = KubernetesStorage::new_unconnected_for_test().with_client_for_test(Client::new(service, "default"));
    assert!(matches!(
        storage.source_count("ConfigMap", None).await,
        Err(ConfWriterError::SourceTimeout)
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn cold_discovery_rejects_forbidden_and_oversize_without_exposing_body() {
    for kind in ["Gateway", "GatewayClass", "HTTPRoute", "ReferenceGrant", "TLSRoute"] {
        for oversized in [false, true] {
            let calls = Arc::new(AtomicUsize::new(0));
            let captured = calls.clone();
            let mut storage = storage(&[], move |request| {
                captured.fetch_add(1, Ordering::Relaxed);
                assert_eq!(request.uri().path(), "/apis/gateway.networking.k8s.io/v1");
                Response::builder()
                    .status(if oversized { 200 } else { 403 })
                    .body(Body::from(if oversized {
                        vec![b'x'; MAX_SOURCE_BYTES + 1]
                    } else {
                        b"sensitive-upstream-error-marker".to_vec()
                    }))
                    .unwrap()
            });
            storage.legacy_versions = LegacySelections::default();
            let error = storage.source_page(kind, None, options(10, None)).await.unwrap_err();
            assert!(!error.to_string().contains("sensitive-upstream-error-marker"));
            if oversized {
                assert!(matches!(error, ConfWriterError::SourceBudget));
            } else {
                assert!(matches!(error, ConfWriterError::PermissionDenied(_)));
            }
            assert_eq!(calls.load(Ordering::Relaxed), 1, "failures must not trigger fallback");
            let error = storage.source_count(kind, None).await.unwrap_err();
            if oversized {
                assert!(matches!(error, ConfWriterError::SourceBudget));
            } else {
                assert!(matches!(error, ConfWriterError::PermissionDenied(_)));
            }
        }
    }
}

#[tokio::test]
async fn exact_source_get_preserves_fields_and_enforces_scope_identity_and_bounds() {
    let source = storage(&["apps"], |request| {
        assert_eq!(request.uri().path(), "/api/v1/namespaces/apps/configmaps/one");
        response(StatusCode::OK, row("apps", "one"))
    });
    assert!(source
        .get_source_one("ConfigMap", Some("apps"), "one")
        .await
        .unwrap()
        .contains("unknownFutureField"));
    assert!(matches!(
        source.get_source_one("ConfigMap", Some("other"), "one").await,
        Err(ConfWriterError::PermissionDenied(_))
    ));
    for (status, body, expected) in [
        (404, b"private-server-marker".to_vec(), 404),
        (200, vec![b'x'; MAX_SOURCE_BYTES + 1], 413),
    ] {
        let source = storage(&[], move |_| {
            Response::builder()
                .status(status)
                .body(Body::from(body.clone()))
                .unwrap()
        });
        let error = source
            .get_source_one("ConfigMap", Some("apps"), "one")
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("private-server-marker"));
        assert!(matches!(
            (&error, expected),
            (ConfWriterError::NotFound(_), 404) | (ConfWriterError::SourceBudget, 413)
        ));
    }
    let source = storage(&[], |_| response(StatusCode::OK, row("other", "one")));
    assert!(source.get_source_one("ConfigMap", Some("apps"), "one").await.is_err());
}

#[tokio::test]
async fn source_cas_uses_one_bounded_get_and_native_versioned_replace() {
    for status in [200, 409, 422] {
        let original = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"namespace":"apps","name":"one","uid":"uid","resourceVersion":"7"},"data":{"unknown":"old"},"status":{"preserve":true}});
        let expected = query::SourcePrecondition::from_value(&original).unwrap();
        let mut requested = original.clone();
        requested["data"]["unknown"] = json!("new");
        requested["metadata"]["resourceVersion"] = json!("incorrect-client-version");
        let reads = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let read_count = reads.clone();
        let write_count = writes.clone();
        let service = service_fn(move |request: Request<Body>| {
            let original = original.clone();
            let reads = read_count.clone();
            let writes = write_count.clone();
            async move {
                assert_eq!(request.uri().path(), "/api/v1/namespaces/apps/configmaps/one");
                if request.method() == http::Method::GET {
                    reads.fetch_add(1, Ordering::Relaxed);
                    return Ok::<_, Infallible>(response(StatusCode::OK, original));
                }
                assert_eq!(request.method(), http::Method::PUT);
                writes.fetch_add(1, Ordering::Relaxed);
                let bytes = axum::body::to_bytes(axum::body::Body::new(request.into_body()), MAX_SOURCE_BYTES)
                    .await
                    .unwrap();
                let sent: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(sent["metadata"]["resourceVersion"], "7");
                assert_eq!(sent["metadata"]["uid"], "uid");
                assert_eq!(sent["status"]["preserve"], true);
                assert_eq!(sent["data"]["unknown"], "new");
                Ok(response(
                    StatusCode::from_u16(status).unwrap(),
                    if status == 200 {
                        sent
                    } else {
                        json!({"message":"sensitive-error-marker"})
                    },
                ))
            }
        });
        let storage =
            KubernetesStorage::new_unconnected_for_test().with_client_for_test(Client::new(service, "default"));
        let result = storage
            .update_one_if_source("ConfigMap", Some("apps"), "one", requested.to_string(), &expected)
            .await;
        match status {
            200 => assert!(result.is_ok()),
            409 => assert!(matches!(result, Err(ConfWriterError::Conflict(_)))),
            _ => assert!(matches!(result, Err(ConfWriterError::ValidationError(_)))),
        }
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert_eq!(writes.load(Ordering::Relaxed), 1);
    }
}
