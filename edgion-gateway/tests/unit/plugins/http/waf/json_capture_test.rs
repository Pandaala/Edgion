use super::*;
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::request_body::{BodyStorage, StorageIoOperation, STORAGE_IO_TEST_HOOK};
use crate::EdgionHttpContext;
use edgion_waf::seclang::{
    compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, Phase,
    PhaseDefaults, SemanticValidationContext,
};
use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use pingora_core::protocols::http::RequestBodyBuffer;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}
fn transaction(rules: &str, mode: RuleEngineMode) -> Transaction<'static> {
    let phases = [
        PhaseDefaults::pass(Phase::RequestHeaders, true, true),
        PhaseDefaults::pass(Phase::RequestBody, true, true),
    ];
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &phases).unwrap();
    let generation = compile_with_includes(
        "capture-test",
        rules.as_bytes(),
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0;
    let headers = [RequestHeader::new(
        b"content-type",
        if rules.contains("XML") {
            b"application/xml".as_slice()
        } else {
            b"application/json".as_slice()
        },
    )];
    let head = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"capture-test",
        &headers,
    );
    let mut tx = Transaction::new_shared_with_modes(
        Arc::new(generation),
        head,
        RuntimeLimits::default(),
        edgion_waf::seclang::EvaluationMode::Accelerated,
        mode,
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx
}
fn context() -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.in_request_body_stage = true;
    ctx.request_info.body_declared = Some(true);
    ctx.request_body_config.memory_buffer_bytes = 1;
    ctx.request_body_config.max_body_size_bytes = 1 << 20;
    ctx
}
struct MetricObservation {
    snapshotter: Snapshotter,
    inspected: std::cell::Cell<u64>,
    runtime: std::cell::Cell<f64>,
}
impl MetricObservation {
    fn new(recorder: &DebuggingRecorder) -> Self {
        Self {
            snapshotter: recorder.snapshotter(),
            inspected: std::cell::Cell::new(0),
            runtime: std::cell::Cell::new(0.0),
        }
    }
}
fn metric(observation: &MetricObservation, name: &str) -> Option<DebugValue> {
    // DebuggingRecorder snapshots drain every value, including gauges. Accumulate
    // their deltas so readiness polling does not change the observed balance.
    for (key, _, _, value) in observation.snapshotter.snapshot().into_vec() {
        match (key.key().name(), value) {
            ("edgion_gateway_waf_body_bytes_inspected_total", DebugValue::Counter(value)) => {
                observation.inspected.set(observation.inspected.get() + value)
            }
            ("edgion_gateway_waf_runtime_bytes_active", DebugValue::Gauge(value)) => {
                observation.runtime.set(observation.runtime.get() + f64::from(value))
            }
            _ => {}
        }
    }
    match name {
        "edgion_gateway_waf_body_bytes_inspected_total" => Some(DebugValue::Counter(observation.inspected.get())),
        "edgion_gateway_waf_runtime_bytes_active" => Some(DebugValue::Gauge(observation.runtime.get().into())),
        _ => None,
    }
}
fn gauge_is_released(snapshotter: &MetricObservation) {
    assert!(
        matches!(metric(snapshotter, "edgion_gateway_waf_runtime_bytes_active"), Some(DebugValue::Gauge(value)) if f64::from(value) == 0.0)
    );
}
type IoCounts = Arc<[AtomicUsize; 2]>;
type IoHook = Arc<dyn Fn(StorageIoOperation) + Send + Sync>;
fn io_counts() -> (IoCounts, IoHook) {
    let counts = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let copied = Arc::clone(&counts);
    let hook = Arc::new(move |operation| {
        if operation == StorageIoOperation::MaterializeRead {
            copied[0].fetch_add(1, Ordering::Relaxed);
        }
        if operation == StorageIoOperation::SnapshotRead {
            copied[1].fetch_add(1, Ordering::Relaxed);
        }
    });
    (counts, hook)
}

#[tokio::test]
async fn fresh_spilled_json_inspects_real_chunks_before_eof_without_read_all() {
    fresh_spilled(false).await;
}

#[tokio::test]
async fn fresh_spilled_xml_inspects_real_chunks_before_eof_without_read_all() {
    fresh_spilled(true).await;
}

async fn fresh_spilled(xml: bool) {
    let recorder = DebuggingRecorder::new();
    let snapshots = MetricObservation::new(&recorder);
    let _metrics = metrics::set_default_local_recorder(&recorder);
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    let mut downstream = pingora_proxy::Session::new_h1(Box::new(server));
    downstream.read_request().await.unwrap();
    let mut ctx = context();
    let rules = if xml {
        "SecRule XML \"@contains blocked\" \"id:1,phase:2,deny\""
    } else {
        "SecRule ARGS \"@contains blocked\" \"id:1,phase:2,deny\""
    };
    let mut tx = transaction(rules, RuleEngineMode::On);
    let first = if xml {
        b"<r a='blocked'>".as_slice()
    } else {
        b"{\"a\":\"blocked\",".as_slice()
    };
    let last = if xml {
        b"safe</r>".as_slice()
    } else {
        b"\"a\":\"safe\"}".as_slice()
    };
    client
        .write_all(format!("{:x}\r\n", first.len()).as_bytes())
        .await
        .unwrap();
    client.write_all(first).await.unwrap();
    client.write_all(b"\r\n").await.unwrap();
    let (counts, hook) = io_counts();
    {
        let mut adapter = PingoraSessionAdapter::new(&mut downstream, &mut ctx);
        let inspection = inspect_structured(
            &mut adapter,
            &mut tx,
            if xml {
                InspectionLimits::Xml(XmlInspectionLimits::default())
            } else {
                InspectionLimits::Json(JsonInspectionLimits::default())
            },
            RuntimeLimits::default(),
            1 << 20,
            true,
        );
        let mut inspection = Box::pin(STORAGE_IO_TEST_HOOK.scope(hook, inspection));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = &mut inspection => panic!("inspection completed before transport EOF: {}", result.is_ok()),
                    () = tokio::task::yield_now() => {}
                }
                if matches!(metric(&snapshots, "edgion_gateway_waf_body_bytes_inspected_total"), Some(DebugValue::Counter(count)) if count == first.len() as u64) { break; }
            }
        }).await.expect("first transport chunk must reach the inspector");
        assert!(
            matches!(metric(&snapshots, "edgion_gateway_waf_body_bytes_inspected_total"), Some(DebugValue::Counter(count)) if count == first.len() as u64)
        );
        assert!(
            matches!(metric(&snapshots, "edgion_gateway_waf_runtime_bytes_active"), Some(DebugValue::Gauge(value)) if f64::from(value) > 0.0),
            "provisional gauge: {:?}",
            metric(&snapshots, "edgion_gateway_waf_runtime_bytes_active")
        );
        client
            .write_all(format!("{:x}\r\n", last.len()).as_bytes())
            .await
            .unwrap();
        client.write_all(last).await.unwrap();
        client.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        assert!(inspection.await.is_ok());
    }
    gauge_is_released(&snapshots);
    assert_eq!(counts[0].load(Ordering::Relaxed), 0);
    assert_eq!(counts[1].load(Ordering::Relaxed), 0);
    assert_eq!(
        tx.execute_request_body()
            .unwrap()
            .intervention()
            .map(|intervention| intervention.status()),
        xml.then_some(403)
    );
    let mut reader = ctx.request_body.snapshot_reader(false).unwrap();
    let mut replay = Vec::new();
    while let Some(chunk) = reader.next_chunk().await.unwrap() {
        replay.extend_from_slice(&chunk);
    }
    assert_eq!(replay, [first, last].concat());
    assert!(ctx.request_body.overwrite(Bytes::from_static(b"different")).is_err());
}

#[tokio::test]
async fn cached_spilled_json_reads_bounded_snapshot_and_rejects_oversize_before_io() {
    cached_spilled(false).await;
}

#[tokio::test]
async fn cached_spilled_xml_capture() {
    cached_spilled(true).await;
}

async fn cached_spilled(xml: bool) {
    for oversized in [false, true] {
        let input = if xml {
            Bytes::from_static(b"<r a='blocked'/>")
        } else {
            Bytes::from_static(br#"{"a":"blocked"}"#)
        };
        let (mut storage, view) = BodyStorage::new(1, 1024);
        storage.write(&input).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = context();
        ctx.request_body.install(view);
        let (_, server) = tokio::io::duplex(4096);
        let mut downstream = pingora_proxy::Session::new_h1(Box::new(server));
        let mut tx = transaction(
            if xml {
                "SecRule XML \"@contains blocked\" \"id:1,phase:2,deny\""
            } else {
                "SecRule ARGS \"@contains blocked\" \"id:1,phase:2,deny\""
            },
            RuleEngineMode::On,
        );
        let (counts, hook) = io_counts();
        let limits = JsonInspectionLimits {
            lexical: edgion_waf::seclang::JsonStreamLimits {
                max_source_bytes: if oversized { input.len() - 1 } else { 1024 },
                ..JsonInspectionLimits::default().lexical
            },
            ..JsonInspectionLimits::default()
        };
        let mut adapter = PingoraSessionAdapter::new(&mut downstream, &mut ctx);
        let result = STORAGE_IO_TEST_HOOK
            .scope(
                hook,
                inspect_structured(
                    &mut adapter,
                    &mut tx,
                    if xml {
                        InspectionLimits::Xml(XmlInspectionLimits {
                            parser: edgion_waf::seclang::XmlStreamLimits {
                                max_source_bytes: limits.lexical.max_source_bytes,
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                    } else {
                        InspectionLimits::Json(limits)
                    },
                    RuntimeLimits::default(),
                    1024,
                    true,
                ),
            )
            .await;
        if oversized {
            if xml {
                assert!(matches!(
                    result,
                    Err(JsonCaptureError::Runtime(RuntimeError::XmlStream {
                        kind: XmlStreamError::Limit {
                            kind: XmlStreamLimit::SourceBytes,
                            ..
                        }
                    }))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(JsonCaptureError::Runtime(RuntimeError::JsonChecker {
                        kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                            kind: JsonStreamLimit::SourceBytes,
                            ..
                        })
                    }))
                ));
            }
            assert_eq!(counts[1].load(Ordering::Relaxed), 0);
        } else {
            assert!(result.is_ok());
            assert_eq!(tx.execute_request_body().unwrap().intervention().unwrap().status(), 403);
            assert!(counts[1].load(Ordering::Relaxed) > 0);
        }
        assert_eq!(counts[0].load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn cancelled_capture_releases_provisional_gauge_and_abandons_transaction() {
    cancelled(false).await;
}

#[tokio::test]
async fn cancelled_xml_capture() {
    cancelled(true).await;
}

async fn cancelled(xml: bool) {
    let recorder = DebuggingRecorder::new();
    let snapshots = MetricObservation::new(&recorder);
    let _metrics = metrics::set_default_local_recorder(&recorder);
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(if xml {
            b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n<r\r\n".as_slice()
        } else {
            b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{\"\r\n".as_slice()
        })
        .await
        .unwrap();
    let mut downstream = pingora_proxy::Session::new_h1(Box::new(server));
    downstream.read_request().await.unwrap();
    let mut ctx = context();
    let mut tx = transaction(
        if xml {
            "SecRule XML \"@contains blocked\" \"id:1,phase:2,pass\""
        } else {
            "SecAction \"id:1,phase:2,pass\""
        },
        RuleEngineMode::On,
    );
    let mut adapter = PingoraSessionAdapter::new(&mut downstream, &mut ctx);
    {
        let inspection = inspect_structured(
            &mut adapter,
            &mut tx,
            if xml {
                InspectionLimits::Xml(XmlInspectionLimits::default())
            } else {
                InspectionLimits::Json(JsonInspectionLimits::default())
            },
            RuntimeLimits::default(),
            1024,
            true,
        );
        let mut inspection = Box::pin(inspection);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = &mut inspection => panic!("inspection completed before transport EOF: {}", result.is_ok()),
                    () = tokio::task::yield_now() => {}
                }
                if matches!(
                    metric(&snapshots, "edgion_gateway_waf_body_bytes_inspected_total"),
                    Some(DebugValue::Counter(2))
                ) {
                    break;
                }
            }
        })
        .await
        .expect("partial chunk must reach the inspector before cancellation");
    }
    gauge_is_released(&snapshots);
    if xml {
        assert_eq!(
            tx.execute_request_body(),
            Err(RuntimeError::XmlStream {
                kind: XmlStreamError::Abandoned
            })
        );
    } else {
        assert_eq!(
            tx.execute_request_body(),
            Err(RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Abandoned)
            })
        );
    }
    assert_eq!(ctx.request_body.cached_len(), Ok(None));
}

#[tokio::test]
async fn latched_json_failure_preserves_capture_error_priority_and_releases_gauge() {
    latched(false).await;
}

#[tokio::test]
async fn latched_xml_capture() {
    latched(true).await;
}

async fn latched(xml: bool) {
    for over_cap in [false, true] {
        let recorder = DebuggingRecorder::new();
        let snapshots = MetricObservation::new(&recorder);
        let _metrics = metrics::set_default_local_recorder(&recorder);
        let (mut client, server) = tokio::io::duplex(4096);
        client
            .write_all(if xml {
                b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n<]\r\n".as_slice()
            } else {
                b"POST / HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{]\r\n".as_slice()
            })
            .await
            .unwrap();
        if over_cap {
            client.write_all(b"8\r\n padding\r\n0\r\n\r\n").await.unwrap();
        }
        client.shutdown().await.unwrap();
        let mut downstream = pingora_proxy::Session::new_h1(Box::new(server));
        downstream.read_request().await.unwrap();
        let mut ctx = context();
        let mut tx = transaction(
            if xml {
                "SecRule XML \"@contains hit\" \"id:1,phase:2,pass\""
            } else {
                "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\""
            },
            RuleEngineMode::On,
        );
        let mut adapter = PingoraSessionAdapter::new(&mut downstream, &mut ctx);
        let result = inspect_structured(
            &mut adapter,
            &mut tx,
            if xml {
                InspectionLimits::Xml(XmlInspectionLimits::default())
            } else {
                InspectionLimits::Json(JsonInspectionLimits::default())
            },
            RuntimeLimits::default(),
            4,
            true,
        )
        .await;
        if over_cap {
            assert!(matches!(
                result,
                Err(JsonCaptureError::Body(BodyReadError::BufferLimitExceeded { limit: 4 }))
            ));
        } else {
            assert!(matches!(result, Err(JsonCaptureError::Body(BodyReadError::Incomplete))));
        }
        gauge_is_released(&snapshots);
        assert!(
            matches!(metric(&snapshots, "edgion_gateway_waf_body_bytes_inspected_total"), Some(DebugValue::Counter(count)) if count <= 2)
        );
        assert_eq!(ctx.request_body.cached_len(), Ok(None));
    }
}

#[tokio::test]
async fn actual_raw_visibility_materializes_once_and_detection_fallback_uses_full_length() {
    raw_visibility(false).await;
}

#[tokio::test]
async fn raw_visibility_xml_capture() {
    raw_visibility(true).await;
}

async fn raw_visibility(xml: bool) {
    for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
        let input = if xml {
            if mode == RuleEngineMode::On {
                Bytes::from_static(b"<r>blocked</r>")
            } else {
                Bytes::from_static(b"<] remaining blocked")
            }
        } else if mode == RuleEngineMode::On {
            Bytes::from_static(br#"{"a":"blocked"}"#)
        } else {
            Bytes::from_static(b"{] remaining blocked")
        };
        let force = if mode == RuleEngineMode::On {
            "SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=On\"\n"
        } else {
            ""
        };
        let rules = format!("{force}SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,pass,setvar:tx.raw=yes\"\nSecRule REQUEST_BODY_LENGTH \"@eq {}\" \"id:3,phase:2,pass,setvar:tx.length=yes\"", input.len());
        let rules = if xml {
            format!("{rules}\nSecRule XML \"@contains blocked\" \"id:4,phase:2,pass\"")
        } else {
            rules
        };
        let (mut storage, view) = BodyStorage::new(1, 1024);
        storage.write(&input).await.unwrap();
        storage.finish().await.unwrap();
        let mut ctx = context();
        ctx.request_body.install(view);
        let (_, server) = tokio::io::duplex(4096);
        let mut downstream = pingora_proxy::Session::new_h1(Box::new(server));
        let mut tx = transaction(&rules, mode);
        let (counts, hook) = io_counts();
        let mut adapter = PingoraSessionAdapter::new(&mut downstream, &mut ctx);
        assert!(STORAGE_IO_TEST_HOOK
            .scope(
                hook,
                inspect_structured(
                    &mut adapter,
                    &mut tx,
                    if xml {
                        InspectionLimits::Xml(XmlInspectionLimits::default())
                    } else {
                        InspectionLimits::Json(JsonInspectionLimits::default())
                    },
                    RuntimeLimits::default(),
                    1024,
                    true
                )
            )
            .await
            .is_ok());
        assert_eq!(counts[0].load(Ordering::Relaxed), 1);
        tx.execute_request_body().unwrap();
        assert_eq!(tx.tx_value(b"raw"), Some(b"yes".as_slice()));
        assert_eq!(tx.tx_value(b"length"), Some(b"yes".as_slice()));
    }
}
