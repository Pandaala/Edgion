#![allow(clippy::field_reassign_with_default)]

use super::*;
use crate::backends::get_roundrobin_store;
use crate::plugins::http::common::http_client::get_mirror_http_client;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::EdgionHttpContext;
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use edgion_resources::resources::http_route::BackendObjectReference;
use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use pingora_core::protocols::http::RequestBodyBuffer;
use std::collections::{BTreeMap, HashMap};

const TEST_NS: &str = "mirror-arming-test";
const TEST_SERVICE: &str = "mirror-arming-backend";
// TEST-NET-1 is only endpoint-selection data. The test writer has no transport.
const TEST_ADDR: &str = "192.0.2.10";
const TEST_PORT: i32 = 8080;

pub(super) enum WriterEvent {
    Prepared {
        method: reqwest::Method,
        url: String,
        headers: HeaderMap,
        snapshot: Option<Bytes>,
    },
    Chunk(Bytes),
    Finished {
        status: &'static str,
        bytes: u64,
        chunks: u64,
    },
}

pub(super) async fn consume_without_network(
    events: &mpsc::UnboundedSender<WriterEvent>,
    method: reqwest::Method,
    url: String,
    headers: HeaderMap,
    uses_snapshot: bool,
    stream: impl tokio_stream::Stream<Item = Result<Bytes, std::io::Error>>,
) -> Result<(), reqwest::Error> {
    tokio::pin!(stream);
    let snapshot = if uses_snapshot {
        let mut data = Vec::new();
        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk.unwrap());
        }
        Some(Bytes::from(data))
    } else {
        None
    };
    events
        .send(WriterEvent::Prepared {
            method,
            url,
            headers,
            snapshot,
        })
        .unwrap();
    if !uses_snapshot {
        while let Some(chunk) = stream.next().await {
            events.send(WriterEvent::Chunk(chunk.unwrap())).unwrap();
        }
    }
    Ok(())
}

fn observe_writer(plugin: &mut RequestMirrorPlugin) -> mpsc::UnboundedReceiver<WriterEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    plugin.writer_events = Some(tx);
    rx
}

async fn next_event(events: &mut mpsc::UnboundedReceiver<WriterEvent>) -> WriterEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn assert_prepared(events: &mut mpsc::UnboundedReceiver<WriterEvent>, snapshot: Option<Bytes>) {
    let WriterEvent::Prepared {
        method,
        url,
        headers,
        snapshot: actual,
    } = next_event(events).await
    else {
        panic!("expected prepared mirror request");
    };
    assert_eq!(method, reqwest::Method::POST);
    assert_eq!(url, format!("http://{TEST_ADDR}:{TEST_PORT}/mirror"));
    assert_eq!(headers["host"], format!("{TEST_ADDR}:{TEST_PORT}"));
    assert_eq!(headers["x-mirror"], "true");
    assert_eq!(actual, snapshot);
    if snapshot.is_some() {
        assert!(!headers.contains_key("content-length"));
        assert!(!headers.contains_key("transfer-encoding"));
    }
}

async fn assert_finished(
    events: &mut mpsc::UnboundedReceiver<WriterEvent>,
    plugin: &RequestMirrorPlugin,
    expected: (&str, u64, u64),
) {
    let WriterEvent::Finished { status, bytes, chunks } = next_event(events).await else {
        panic!("expected completed mirror writer");
    };
    assert_eq!((status, bytes, chunks), expected);
    assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
    assert!(events.try_recv().is_err());
}

/// Publish the shared immutable fixture once: concurrent replacements of
/// the same service can transiently invalidate endpoint selection.
fn insert_test_endpoint_slice() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(publish_test_endpoint_slice);
}

fn publish_test_endpoint_slice() {
    let mut labels = BTreeMap::new();
    labels.insert(SERVICE_NAME.to_string(), TEST_SERVICE.to_string());
    let slice = EndpointSlice {
        metadata: ObjectMeta {
            name: Some(format!("{TEST_SERVICE}-slice")),
            namespace: Some(TEST_NS.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        endpoints: vec![Endpoint {
            addresses: vec![TEST_ADDR.to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ports: Some(vec![EndpointPort {
            port: Some(TEST_PORT),
            ..Default::default()
        }]),
    };
    let mut add = HashMap::new();
    add.insert(format!("{TEST_NS}/{TEST_SERVICE}-slice"), slice);
    get_roundrobin_store().update_data_only(add, HashMap::new(), &Default::default());
}

fn test_plugin_with_tuning(mut tuning: MirrorTuning) -> RequestMirrorPlugin {
    // The mirror access log needs process-wide log infrastructure that a
    // unit test does not bootstrap.
    tuning.mirror_log = false;
    owned_plugin(
        HTTPRequestMirrorFilter {
            backend_ref: BackendObjectReference {
                group: Some(String::new()),
                kind: None,
                name: TEST_SERVICE.to_string(),
                namespace: Some(TEST_NS.to_string()),
                port: Some(TEST_PORT),
                ref_denied: None,
            },
            fraction: None,
            percent: None,
        },
        tuning,
    )
}

fn owned_plugin(config: HTTPRequestMirrorFilter, tuning: MirrorTuning) -> RequestMirrorPlugin {
    let route: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({"metadata":{"name":"mirror-owner","namespace":TEST_NS},"spec":{"rules":[{"filters":[{"type":"RequestMirror","requestMirror":config}]}]}})).unwrap();
    let field = edgion_resources::resources::refs::route_fields::HttpRouteMirrorRefs::from_route(&route)
        .unwrap()
        .effective_at(0, None, 0)
        .unwrap();
    RequestMirrorPlugin::new(field, tuning).unwrap()
}

fn test_plugin() -> RequestMirrorPlugin {
    test_plugin_with_tuning(MirrorTuning::default())
}

#[test]
fn different_plugin_generations_share_the_process_mirror_client() {
    let before = get_mirror_http_client();

    let mut first_tuning = MirrorTuning::default();
    first_tuning.connect_timeout_ms = 11;
    first_tuning.write_timeout_ms = 17;
    let _first = test_plugin_with_tuning(first_tuning);
    let after_first = get_mirror_http_client();

    let mut second_tuning = MirrorTuning::default();
    second_tuning.connect_timeout_ms = 23;
    second_tuning.write_timeout_ms = 29;
    let _second = test_plugin_with_tuning(second_tuning);
    let after_second = get_mirror_http_client();

    assert!(std::ptr::eq(before, after_first));
    assert!(std::ptr::eq(before, after_second));
}

fn test_session(headers_end_stream: Option<bool>) -> MockPluginSession {
    test_session_with_context(headers_end_stream, EdgionHttpContext::new(), &[])
}

fn test_session_with_context(
    headers_end_stream: Option<bool>,
    ctx: EdgionHttpContext,
    headers: &[(&str, &str)],
) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(ctx);
    session
        .expect_request_headers_end_stream()
        .return_const(headers_end_stream);
    session.expect_declares_body().never();
    session.expect_get_path().return_const("/mirror".to_string());
    session.expect_get_query().return_const(None);
    session.expect_get_method().return_const("POST".to_string());
    setup_header_mocks(&mut session, headers);
    session
}

#[tokio::test]
async fn disabled_sampling_and_saturated_concurrency_do_not_start_writer() {
    for disabled in [true, false] {
        let mut plugin = test_plugin();
        let mut events = observe_writer(&mut plugin);
        let held = if disabled {
            plugin.config.percent = Some(0);
            None
        } else {
            Some(
                plugin
                    .semaphore
                    .clone()
                    .try_acquire_many_owned(plugin.tuning.max_concurrent as u32)
                    .unwrap(),
            )
        };
        // Any endpoint/session access would fail an unmet mock expectation.
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("RequestMirror");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            log.render_for_test(),
            if disabled {
                "mirror disabled"
            } else {
                "mirror skipped concurrency"
            }
        );
        assert!(events.try_recv().is_err());
        drop(held);
        assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
    }
}

#[tokio::test]
async fn denied_or_wrong_kind_mirror_never_arms_or_uses_session() {
    for (denied, group, kind, diagnostic) in [
        (true, "", "Service", "mirror ref denied"),
        (false, "example.com", "Service", "mirror invalid kind"),
        (false, "", "Secret", "mirror invalid kind"),
    ] {
        let mut config = test_plugin().config;
        config.backend_ref.group = Some(group.to_string());
        config.backend_ref.kind = Some(kind.to_string());
        config.backend_ref.ref_denied = denied.then(|| edgion_resources::resources::common::RefDenied {
            target_namespace: TEST_NS.to_string(),
            target_name: TEST_SERVICE.to_string(),
            reason: None,
        });
        let mut plugin = owned_plugin(config, MirrorTuning::default());
        let mut events = observe_writer(&mut plugin);
        // No session expectation is installed: even reading the request
        // context or preparing its body before rejecting fails this test.
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("RequestMirror");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(log.render_for_test().contains(diagnostic));
        assert!(events.try_recv().is_err());
        assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
    }
}

#[tokio::test]
async fn committed_snapshot_has_independent_body_and_framing() {
    insert_test_endpoint_slice();
    let mut plugin = test_plugin();
    let mut events = observe_writer(&mut plugin);
    let (mut storage, view) = crate::request_body::BodyStorage::new(1024, 1024);
    storage.write(&Bytes::from_static(b"original")).await.unwrap();
    storage.finish().await.unwrap();
    view.overwrite_body(Bytes::from_static(b"committed")).unwrap();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body.install(view.clone());
    let mut session = test_session_with_context(
        Some(false),
        ctx,
        &[("content-length", "8"), ("transfer-encoding", "chunked")],
    );
    let snapshot = view.snapshot_reader().unwrap();
    session
        .expect_request_body_snapshot_reader()
        .withf(|original| !*original)
        .times(1)
        .return_once(move |_| Ok(snapshot));
    session.expect_set_mirror_state().never();
    let mut log = PluginLog::new("RequestMirror");

    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(log.render_for_test(), "mirror sent");
    view.overwrite_body(Bytes::from_static(b"later")).unwrap();
    assert_prepared(&mut events, Some(Bytes::from_static(b"committed"))).await;
    assert_finished(&mut events, &plugin, ("ok", 9, 1)).await;
    assert_eq!(view.read_all().await.unwrap(), "later");
}

#[tokio::test]
async fn headers_end_stream_does_not_install_armed_streaming_state() {
    insert_test_endpoint_slice();
    let mut plugin = test_plugin();
    let mut events = observe_writer(&mut plugin);
    let mut session = test_session(Some(true));
    // A transport-level header END_STREAM proves the empty body complete,
    // so no armed mirror state is installed.
    session.expect_set_mirror_state().never();
    let mut log = PluginLog::new("RequestMirror");

    let result = plugin.run_request(&mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "mirror sent");
    assert_prepared(&mut events, None).await;
    assert_finished(&mut events, &plugin, ("ok", 0, 0)).await;
}

#[tokio::test]
async fn headers_without_end_stream_keep_armed_streaming_state() {
    insert_test_endpoint_slice();
    let mut plugin = test_plugin();
    let mut events = observe_writer(&mut plugin);
    let mut session = test_session(Some(false));
    // This is the H2 `Content-Length: 0` without END_STREAM shape: header
    // declarations cannot replace the transport fact. It also proves the
    // fixture resolves a real mirror target; otherwise the positive test
    // above could pass because of an early mirror-resolution return.
    let (state_tx, state_rx) = oneshot::channel();
    session.expect_set_mirror_state().times(1).return_once(|state| {
        assert!(matches!(state, MirrorState::Streaming { .. }));
        assert!(state_tx.send(state).is_ok());
        true
    });
    let mut log = PluginLog::new("RequestMirror");

    let result = plugin.run_request(&mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "mirror sent");
    assert_prepared(&mut events, None).await;
    assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent - 1);
    let MirrorState::Streaming {
        body_tx,
        writer_handle,
        cancellation,
        ..
    } = state_rx.await.unwrap()
    else {
        panic!("expected streaming mirror");
    };
    body_tx.send(Ok(Bytes::from_static(b"streamed body"))).await.unwrap();
    let WriterEvent::Chunk(body) = next_event(&mut events).await else {
        panic!("expected streamed body chunk");
    };
    assert_eq!(body, "streamed body");
    cancellation.complete();
    drop(body_tx);
    writer_handle.await.unwrap();
    assert_finished(&mut events, &plugin, ("ok", 13, 1)).await;
}

#[tokio::test]
async fn unknown_headers_end_stream_keeps_armed_streaming_state() {
    insert_test_endpoint_slice();
    let mut plugin = test_plugin();
    let mut events = observe_writer(&mut plugin);
    let mut session = test_session(None);
    // `None` means the session cannot provide the transport fact: stay
    // conservative.
    let (state_tx, state_rx) = oneshot::channel();
    session.expect_set_mirror_state().times(1).return_once(|state| {
        assert!(matches!(state, MirrorState::Streaming { .. }));
        assert!(state_tx.send(state).is_ok());
        true
    });
    let mut log = PluginLog::new("RequestMirror");

    let result = plugin.run_request(&mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "mirror sent");
    assert_prepared(&mut events, None).await;
    drop(state_rx.await.unwrap());
    assert_finished(&mut events, &plugin, ("body_abandoned", 0, 0)).await;
}

#[tokio::test]
async fn frozen_body_stage_refuses_streaming_mirror_install() {
    insert_test_endpoint_slice();
    let mut plugin = test_plugin();
    let mut events = observe_writer(&mut plugin);
    let mut session = test_session(Some(false));
    session.expect_set_mirror_state().times(1).return_once(|state| {
        assert!(matches!(state, MirrorState::Streaming { .. }));
        false
    });
    let mut log = PluginLog::new("RequestMirror");

    let result = plugin.run_request(&mut session, &mut log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "err: mirror body stage frozen");
    // Cancellation is already ready and wins without polling the body/send future.
    assert_finished(&mut events, &plugin, ("body_abandoned", 0, 0)).await;
}

#[tokio::test]
async fn invalid_mirror_targets_are_cold_and_never_use_session_or_writer() {
    for backend in [
        serde_json::json!({"name":"bad/name","namespace":TEST_NS,"port":TEST_PORT}),
        serde_json::json!({"name":TEST_SERVICE,"namespace":"bad/ns","port":TEST_PORT}),
        serde_json::json!({"name":TEST_SERVICE,"namespace":TEST_NS,"group":"core","port":TEST_PORT}),
        serde_json::json!({"name":TEST_SERVICE,"namespace":TEST_NS,"port":0}),
    ] {
        let config = serde_json::from_value(serde_json::json!({"backendRef":backend})).unwrap();
        let mut plugin = owned_plugin(config, MirrorTuning::default());
        assert!(plugin.target.is_err());
        let mut events = observe_writer(&mut plugin);
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("RequestMirror");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(events.try_recv().is_err());
        assert_eq!(plugin.semaphore.available_permits(), plugin.tuning.max_concurrent);
    }
    insert_test_endpoint_slice();
    let mut plugin = test_plugin();
    let key = plugin.target.as_ref().ok().unwrap().service_key.clone();
    assert_eq!(key, format!("{TEST_NS}/{TEST_SERVICE}"));
    // The runtime target is immutable even if diagnostic config is changed in this private test.
    plugin.config.backend_ref.name = "decoy".into();
    assert!(plugin.resolve_target(true).is_ok());
    assert_eq!(plugin.target.as_ref().ok().unwrap().service_key, key);
}
