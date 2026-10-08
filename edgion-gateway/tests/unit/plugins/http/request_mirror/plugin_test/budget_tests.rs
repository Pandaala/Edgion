//! RequestMirror budget regressions for `plugin.rs` (see `super`).
use super::*;
use edgion_resources::resources::EdgionPlugins;
use pingora_core::protocols::http::RequestBodyBuffer;

fn filter() -> HTTPRequestMirrorFilter {
    serde_json::from_value(serde_json::json!({ "backendRef": { "name": "mirror", "port": 8080 } })).unwrap()
}

fn owned_plugin(tuning: MirrorTuning) -> RequestMirrorPlugin {
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({"metadata":{"name":"mirror-owner","namespace":"default"},"spec":{"rules":[{"filters":[{"type":"RequestMirror","requestMirror":filter()}]}]}})).unwrap();
    let field = edgion_resources::resources::refs::route_fields::HttpRouteMirrorRefs::from_route(&route)
        .unwrap()
        .effective_at(0, None, 0)
        .unwrap();
    RequestMirrorPlugin::new(field, tuning).unwrap()
}

#[test]
fn mirror_direct_construction_normalizes_before_tokio_allocation() {
    for value in [0, usize::MAX] {
        let tuning = MirrorTuning {
            max_concurrent: value,
            max_buffered_chunks: value,
            max_buffered_bytes: "999999999999999999999GiB".to_string(),
            ..Default::default()
        };
        let plugin = owned_plugin(tuning);
        assert_eq!(plugin.semaphore.available_permits(), 1024);
        assert_eq!(plugin.max_buffered_bytes, 1024 * 1024);
        let (_tx, _rx) = mpsc::channel::<Bytes>(plugin.mirror_config().max_buffered_chunks);
    }
    let tuning = MirrorTuning {
        max_concurrent: MirrorTuning::MAX_CONCURRENT,
        max_buffered_chunks: MirrorTuning::MAX_BUFFERED_CHUNKS,
        max_buffered_bytes: MirrorTuning::MAX_BUFFERED_BYTES.to_string(),
        ..Default::default()
    };
    let plugin = owned_plugin(tuning);
    assert_eq!(plugin.semaphore.available_permits(), MirrorTuning::MAX_CONCURRENT);
    let (_tx, _rx) = mpsc::channel::<Bytes>(plugin.mirror_config().max_buffered_chunks);
    let budget = plugin.budget.body_budget(plugin.max_buffered_bytes);
    assert!(budget.try_copy(b"a").is_some());
}

#[test]
fn mirror_controller_gateway_validation_matches_at_all_capacity_boundaries() {
    for (field, values) in [
        (
            "maxConcurrent",
            vec![
                serde_json::json!(0),
                serde_json::json!(1),
                serde_json::json!(MirrorTuning::MAX_CONCURRENT),
                serde_json::json!(usize::MAX),
            ],
        ),
        (
            "maxBufferedChunks",
            vec![
                serde_json::json!(0),
                serde_json::json!(1),
                serde_json::json!(MirrorTuning::MAX_BUFFERED_CHUNKS),
                serde_json::json!(usize::MAX),
            ],
        ),
        (
            "maxBufferedBytes",
            vec![
                serde_json::json!("0"),
                serde_json::json!("1B"),
                serde_json::json!(MirrorTuning::MAX_BUFFERED_BYTES.to_string()),
                serde_json::json!("999999999999999999999GiB"),
            ],
        ),
    ] {
        for value in values {
            let mut config = serde_json::json!({"backendRef": {"name": "mirror", "port": 8080}});
            config[field] = value.clone();
            let resource: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins", "metadata": {"name": "mirror"},
                "spec": {"requestPlugins": [{"type": "RequestMirror", "config": config}]}
            }))
            .unwrap();
            let controller = resource.validate_plugins();
            let gateway = crate::plugins::http::plugin_store::compile_plugins(&resource);
            assert_eq!(controller, gateway.preparse_errors, "{field}: {value}");
            assert_eq!(gateway.is_activatable, controller.is_empty());
        }
    }
}

#[tokio::test]
async fn mirror_old_generation_holds_global_capacity_without_blocking_main_request() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let shared = MirrorBudget::new(1, 8).unwrap();
    let mut old = owned_plugin(MirrorTuning::default());
    let mut new = owned_plugin(MirrorTuning::default());
    assert!(Arc::ptr_eq(&old.budget, &new.budget));
    old.budget = Arc::clone(&shared);
    new.budget = shared;
    let in_flight = old.budget.try_acquire_task().unwrap();
    drop(old);
    // No session method or network call is allowed after failed global admission.
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("RequestMirror");
    assert_eq!(
        new.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(new.semaphore.available_permits(), 1024);
    drop(in_flight);
    assert!(new.budget.try_acquire_task().is_some());
}

async fn captured(
    memory_limit: usize,
    body: &[u8],
) -> (
    crate::request_body::BodyStorage,
    Arc<crate::request_body::BodyStorageView>,
) {
    let (mut storage, view) = crate::request_body::BodyStorage::new(memory_limit, 1024 * 1024);
    storage.write(&Bytes::copy_from_slice(body)).await.unwrap();
    storage.finish().await.unwrap();
    (storage, view)
}

fn stream_snapshot(
    body: PreparedSnapshot,
    budget: &MirrorBudget,
    chunk_limit: usize,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    snapshot_stream(
        body,
        budget.body_budget(chunk_limit),
        Arc::new(std::sync::atomic::AtomicU8::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    )
}

#[tokio::test]
async fn mirror_snapshot_and_transport_references_release_their_own_charges() {
    let (_storage, view) = captured(64, b"original").await;
    let budget = MirrorBudget::new(1, 16).unwrap();
    let body = prepare_snapshot(view.snapshot_reader().unwrap(), &budget).unwrap();
    view.overwrite_body(Bytes::from_static(b"changed")).unwrap();
    let mut stream = Box::pin(stream_snapshot(body, &budget, 8));
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk, "original");
    assert!(budget.try_reserve_snapshot(1).is_none());
    assert!(stream.next().await.is_none());
    // Reader memory is gone but the queued/transport payload remains charged.
    assert!(budget.try_reserve_snapshot(8).is_some());
    assert!(budget.try_reserve_snapshot(9).is_none());
    drop(chunk);
    assert!(budget.try_reserve_snapshot(16).is_some());
    assert_eq!(view.read_all().await.unwrap(), "changed");
}

#[tokio::test]
async fn mirror_snapshot_budget_rejection_does_not_modify_main_body() {
    let (_storage, view) = captured(64, b"original").await;
    let budget = MirrorBudget::new(1, 7).unwrap();
    assert!(matches!(
        prepare_snapshot(view.snapshot_reader().unwrap(), &budget),
        Err("mirror skipped buffer limit")
    ));
    assert_eq!(view.read_all().await.unwrap(), "original");
    assert!(budget.try_reserve_snapshot(7).is_some());
}

#[tokio::test]
async fn mirror_disk_snapshot_streams_more_than_the_retained_budget() {
    let raw = vec![b'x'; 192 * 1024];
    let (_storage, view) = captured(1, &raw).await;
    let budget = MirrorBudget::new(1, 64 * 1024).unwrap();
    let body = prepare_snapshot(view.snapshot_reader().unwrap(), &budget).unwrap();
    let mut stream = Box::pin(stream_snapshot(body, &budget, 64 * 1024));
    let mut total = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        assert!(chunk.len() <= 64 * 1024);
        total += chunk.len();
        drop(chunk);
    }
    assert_eq!(total, raw.len());
    assert!(budget.try_reserve_snapshot(64 * 1024).is_some());
}

#[tokio::test]
async fn mirror_snapshot_exhaustion_returns_error_instead_of_clean_eof() {
    let (_storage, view) = captured(1, b"original").await;
    let budget = MirrorBudget::new(1, 7).unwrap();
    let body = prepare_snapshot(view.snapshot_reader().unwrap(), &budget).unwrap();
    let reason = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let mut stream = Box::pin(snapshot_stream(
        body,
        budget.body_budget(7),
        Arc::clone(&reason),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    ));
    assert!(stream.next().await.unwrap().is_err());
    assert_eq!(
        MirrorCancellation::load_reason(&reason),
        Some(MirrorAbandonReason::BufferLimit)
    );
    assert!(budget.try_reserve_snapshot(7).is_some());
}
