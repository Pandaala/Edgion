use super::super::budget::Reservation;
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use edgion_resources::grpc_transcoding::GrpcJsonTranscoderConfig;
use pingora_proxy::{ResponseHeadBoundary, ResponseHeadBoundaryAction, ResponseHeadSource};
use tokio::sync::Semaphore;
fn processor(streaming: bool, format: GrpcJsonStreamFormat) -> GrpcResponseProcessor {
    let descriptor = include_bytes!("../../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb");
    let mut config: GrpcJsonTranscoderConfig = serde_json::from_value(serde_json::json!({ "descriptor":{"type":"inline","base64":STANDARD.encode(descriptor)},"services":["test.TestService"] })).unwrap();
    config.stream_format = format;
    processor_from_config(config, "GET", if streaming { "/v1/numbers" } else { "/v1/hello/name" })
}
fn processor_from_config(config: GrpcJsonTranscoderConfig, method: &str, path: &str) -> GrpcResponseProcessor {
    let compiled = Arc::new(config.compile().unwrap());
    let binding = compiled.bind(method, path, None).unwrap().binding;
    let reservation = Reservation::acquire_from(&Arc::new(Semaphore::new(1))).unwrap();
    let plan = Arc::new(GrpcTranscodingPlan::new(compiled, binding, reservation, 5));
    let mut processor = GrpcResponseProcessor::new(plan);
    let mut header = ResponseHeader::build(200, None).unwrap();
    header.insert_header("content-type", "application/grpc+proto").unwrap();
    header.insert_header("access-control-allow-origin", "*").unwrap();
    header.set_version(http::Version::HTTP_2);
    processor.on_response_header(&mut header);
    processor.plan.head_plan(ResponseHeadSource::Origin, &header).unwrap();
    processor
}
fn frame(payload: &[u8]) -> Bytes {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    Bytes::from(bytes)
}
fn trailers(code: &str) -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", code.parse().unwrap());
    trailers
}

#[test]
fn status_accepts_only_canonical_codes_without_allocating() {
    for code in 0u8..=16 {
        let headers = trailers(&code.to_string());
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| status(&headers));
        assert_eq!(result, Ok(Some(code)));
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
    for code in 0u8..=99 {
        let headers = trailers(&format!("{code:02}"));
        let expected = if (10..=16).contains(&code) {
            Ok(Some(code))
        } else {
            Err(())
        };
        assert_eq!(status(&headers), expected);
    }
    for text in ["", "+0", "-0", " 0", "0 ", "0a", "000", "100", "256"] {
        assert_eq!(status(&trailers(text)), Err(()));
    }
    assert_eq!(status(&HeaderMap::new()), Ok(None));
    let mut opaque = HeaderMap::new();
    opaque.insert("grpc-status", http::HeaderValue::from_bytes(&[0xff]).unwrap());
    assert_eq!(status(&opaque), Err(()));
    let mut duplicate = trailers("0");
    duplicate.append("grpc-status", http::HeaderValue::from_static("0"));
    assert_eq!(status(&duplicate), Err(()));
}

#[tokio::test]
async fn streaming_fragmented_frames_and_real_status_emit_one_terminal() {
    let mut p = processor(true, GrpcJsonStreamFormat::Ndjson);
    let input = frame(&[8, 3]);
    assert!(p.feed(input.slice(..2)).unwrap().is_empty());
    assert!(p.feed(input.slice(2..6)).unwrap().is_empty());
    assert_eq!(
        p.feed(input.slice(6..)).unwrap(),
        b"{\"type\":\"message\",\"data\":{\"number\":3}}\n"
    );
    assert!(p.plan.terminal.lock().is_none());
    p.on_actual_upstream_trailers(Some(&trailers("0")), false)
        .await
        .unwrap();
    assert_eq!(
        p.plan.terminal.lock().take().unwrap(),
        b"{\"type\":\"end\"}\n".as_slice()
    );
    p.on_actual_upstream_trailers(Some(&trailers("0")), false)
        .await
        .unwrap();
    assert!(p.plan.terminal.lock().is_none());
}
#[tokio::test]
async fn streaming_coalesced_sse_and_missing_status_are_distinct() {
    let mut p = processor(true, GrpcJsonStreamFormat::Sse);
    let mut input = frame(&[8, 1]).to_vec();
    input.extend_from_slice(&frame(&[8, 2]));
    assert_eq!(
        p.feed(Bytes::from(input)).unwrap(),
        b"event: rpc_message\ndata: {\"number\":1}\n\nevent: rpc_message\ndata: {\"number\":2}\n\n"
    );
    p.on_actual_upstream_trailers(None, false).await.unwrap();
    let terminal = p.plan.terminal.lock().take().unwrap();
    assert!(terminal.starts_with(b"event: rpc_error\n"));
    assert!(!terminal.windows(7).any(|v| v == b"rpc_end"));
}
#[tokio::test]
async fn unary_waits_for_real_status_and_preserves_post_onion_metadata() {
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    assert!(p.feed(frame(&[10, 2, b'o', b'k'])).unwrap().is_empty());
    assert!(p.plan.ready.lock().is_none());
    p.on_actual_upstream_trailers(Some(&trailers("0")), false)
        .await
        .unwrap();
    let ResponseHeadBoundaryAction::Replace(replacement) =
        p.plan.boundary(ResponseHeadBoundary::CleanTerminalWithoutDecision)
    else {
        panic!("missing unary replacement");
    };
    assert_eq!(replacement.header().status.as_u16(), 200);
    assert_eq!(replacement.header().version, http::Version::HTTP_11);
    assert_eq!(replacement.header().headers["access-control-allow-origin"], "*");
    assert_eq!(replacement.body()[0], br#"{"message":"ok"}"#.as_slice());
    assert!(p.plan.ready.lock().is_none());
}
#[tokio::test]
async fn unary_missing_status_extra_frames_truncation_and_expiration_fail_closed() {
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    p.feed(frame(&[])).unwrap();
    assert!(p.on_actual_upstream_trailers(None, false).await.is_err());
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    p.feed(frame(&[])).unwrap();
    assert!(p.feed(frame(&[])).is_err());
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    p.feed(Bytes::from_static(&[0, 0, 0])).unwrap();
    assert!(p
        .on_actual_upstream_trailers(Some(&trailers("0")), false)
        .await
        .is_err());
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    p.feed(frame(&[])).unwrap();
    p.on_actual_upstream_trailers(Some(&trailers("0")), false)
        .await
        .unwrap();
    assert!(matches!(
        p.plan.boundary(ResponseHeadBoundary::Timeout),
        ResponseHeadBoundaryAction::Fail(_)
    ));
    assert!(p.plan.ready.lock().is_none());
}
#[tokio::test]
async fn header_only_error_maps_to_rest_and_malformed_status_never_succeeds() {
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    p.header_status = Ok(Some(7));
    p.on_actual_upstream_trailers(None, true).await.unwrap();
    let ResponseHeadBoundaryAction::Replace(reply) =
        p.plan.boundary(ResponseHeadBoundary::CleanTerminalWithoutDecision)
    else {
        panic!();
    };
    assert_eq!(reply.header().status.as_u16(), 403);
    assert_eq!(reply.body()[0], ERROR_JSON);
    for code in ["-1", "17", "", "0a"] {
        assert!(status(&trailers(code)).is_err());
    }
    let mut duplicate = trailers("0");
    duplicate.append("grpc-status", "0".parse().unwrap());
    assert!(status(&duplicate).is_err());
    let mut p = processor(true, GrpcJsonStreamFormat::Ndjson);
    assert!(p.feed(Bytes::from_static(&[1, 0, 0, 0, 0])).is_err());
    assert!(p.feed(Bytes::from_static(&[0, 0, 16, 0, 0])).is_err());
}

#[tokio::test]
async fn immediate_stream_credit_exhaustion_cancels_the_transport() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    for malformed in [false, true] {
        let mut p = processor(true, GrpcJsonStreamFormat::Ndjson);
        let queued = p.plan.output(vec![0; 1024 * 1024]).unwrap();
        let mut chunk = Some(if malformed {
            Bytes::from_static(&[1, 0, 0, 0, 0])
        } else {
            frame(&[8, 1])
        });
        let mut sink = ResponseBodySink::new();
        p.on_event(
            &mut chunk,
            StreamingResponseEvent::Data { end_of_stream: false },
            ResponseStreamKind::None,
            &mut sink,
            &mut MockPluginSession::new(),
        )
        .await;
        assert!(sink.is_terminated());
        assert!(chunk.is_none());
        assert!(p.finished);
        assert!(p.plan.terminal.lock().is_none());
        drop(queued);
    }
}

#[tokio::test]
async fn mutable_companions_cannot_manufacture_transport_success() {
    for original in [Some("7"), None, Some("invalid")] {
        let mut actual = original.map(trailers).unwrap_or_default();
        let evidence = status_evidence(&actual);
        actual.insert("grpc-status", "0".parse().unwrap());
        let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
        p.feed(frame(&[10, 2, b'o', b'k'])).unwrap();
        let result = p.on_actual_upstream_trailers(Some(&evidence), false).await;
        if original == Some("7") {
            result.unwrap();
            let ResponseHeadBoundaryAction::Replace(reply) =
                p.plan.boundary(ResponseHeadBoundary::CleanTerminalWithoutDecision)
            else {
                panic!();
            };
            assert_eq!(reply.header().status.as_u16(), 403);
        } else {
            assert!(result.is_err());
        }
    }
}

#[tokio::test]
async fn correction_late_conflict_cannot_publish_ready_unary_output() {
    let mut p = processor(false, GrpcJsonStreamFormat::Ndjson);
    p.feed(frame(&[10, 2, b'o', b'k'])).unwrap();
    p.on_actual_upstream_trailers(Some(&trailers("0")), false)
        .await
        .unwrap();
    assert!(p.plan.ready.lock().is_some());
    p.plan.reject_mutation();
    assert!(matches!(
        p.plan.boundary(ResponseHeadBoundary::CleanTerminalWithoutDecision),
        ResponseHeadBoundaryAction::Fail(_)
    ));
    assert!(p.plan.ready.lock().is_none());
    p.finalize_incomplete("cancelled", "cleanup");
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.stage_logs_mut().push(crate::plugins::runtime::log::StageLogs {
        stage: "request",
        filters: vec![crate::plugins::runtime::PluginLog::new("GrpcJsonTranscoder")],
        edgion_plugins: vec![],
    });
    p.plan.finalize_log(&mut ctx);
    p.plan.finalize_log(&mut ctx);
    let log = serde_json::to_string(ctx.stage_logs_mut()).unwrap();
    assert_eq!(log.matches("conversion owner conflict").count(), 1);
    assert_eq!(ctx.request_info.grpc_status(), Some(0));
}

#[tokio::test]
async fn correction_terminal_outcome_survives_unconditional_cleanup() {
    for (code, expected) in [
        (Some("0"), "exchange completed"),
        (Some("7"), "upstream status failed"),
        (None, "terminal status invalid"),
    ] {
        let mut p = processor(true, GrpcJsonStreamFormat::Ndjson);
        p.feed(frame(&[8, 1])).unwrap();
        let evidence = code.map(trailers);
        p.on_actual_upstream_trailers(evidence.as_ref(), false).await.unwrap();
        p.finalize_incomplete("cancelled", "cleanup");
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stage_logs_mut().push(crate::plugins::runtime::log::StageLogs {
            stage: "request",
            filters: vec![crate::plugins::runtime::PluginLog::new("GrpcJsonTranscoder")],
            edgion_plugins: vec![],
        });
        p.plan.finalize_log(&mut ctx);
        let log = serde_json::to_string(ctx.stage_logs_mut()).unwrap();
        assert!(log.contains(expected), "{log}");
        assert!(!log.contains("interrupted"));
        assert_eq!(ctx.request_info.grpc_status(), code.map(|v| v.parse().unwrap()));
    }
}

#[test]
fn correction_response_header_conflict_rejects_commit_and_processing() {
    for streaming in [false, true] {
        let mut p = processor(streaming, GrpcJsonStreamFormat::Ndjson);
        p.plan.reject_mutation();
        let mut header = ResponseHeader::build(200, None).unwrap();
        header
            .insert_header("content-type", media(GrpcJsonStreamFormat::Ndjson, streaming))
            .unwrap();
        assert!(p.plan.head_plan(ResponseHeadSource::Origin, &header).is_err());
        assert!(p.feed(frame(&[])).is_err());
    }
}

#[tokio::test]
async fn correction_stream_failures_keep_the_first_bounded_diagnostic() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    for (mode, expected) in [
        (0, "response framing invalid"),
        (1, "exchange deadline expired"),
        (2, "output budget exhausted"),
        (3, "conversion owner conflict"),
    ] {
        let mut p = processor(true, GrpcJsonStreamFormat::Ndjson);
        let queued = if mode == 2 {
            Some(p.plan.output(vec![0; 1024 * 1024]).unwrap())
        } else {
            None
        };
        if mode == 1 {
            Arc::get_mut(&mut p.plan).unwrap().deadline = std::time::Instant::now() - std::time::Duration::from_secs(1);
        }
        if mode == 3 {
            p.plan.reject_mutation();
        }
        let mut chunk = Some(if mode == 0 {
            Bytes::from_static(&[1, 0, 0, 0, 0])
        } else {
            frame(&[8, 1])
        });
        let mut sink = ResponseBodySink::new();
        p.on_event(
            &mut chunk,
            StreamingResponseEvent::Data { end_of_stream: false },
            ResponseStreamKind::None,
            &mut sink,
            &mut MockPluginSession::new(),
        )
        .await;
        assert!(sink.is_terminated());
        p.finalize_incomplete("cancelled", "cleanup");
        let mut ctx = crate::EdgionHttpContext::default();
        ctx.stage_logs_mut().push(crate::plugins::runtime::log::StageLogs {
            stage: "request",
            filters: vec![crate::plugins::runtime::PluginLog::new("GrpcJsonTranscoder")],
            edgion_plugins: vec![],
        });
        p.plan.finalize_log(&mut ctx);
        let log = serde_json::to_string(ctx.stage_logs_mut()).unwrap();
        assert_eq!(log.matches(expected).count(), 1, "{log}");
        drop(queued);
    }
}

thread_local! {
    static CALLBACK_WRITERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
pub(super) fn callback_writer() -> CappedWriter {
    CALLBACK_WRITERS.with(|count| count.set(count.get() + 1));
    CappedWriter::new(1024 * 1024)
}
fn callback_writers() -> usize {
    CALLBACK_WRITERS.with(std::cell::Cell::get)
}

#[test]
fn callback_writer_follows_completed_stream_output() {
    let mut unary = processor(false, GrpcJsonStreamFormat::Ndjson);
    let before = callback_writers();
    assert!(unary.feed(Bytes::new()).unwrap().is_empty());
    let input = frame(&[10, 2, b'o', b'k']);
    for byte in 0..input.len() {
        assert!(unary.feed(input.slice(byte..byte + 1)).unwrap().is_empty());
    }
    assert_eq!(callback_writers() - before, 0);

    for format in [GrpcJsonStreamFormat::Ndjson, GrpcJsonStreamFormat::Sse] {
        let mut p = processor(true, format);
        let before = callback_writers();
        assert!(p.feed(Bytes::new()).unwrap().is_empty());
        let input = frame(&[8, 3]);
        for byte in 0..input.len() - 1 {
            assert!(p.feed(input.slice(byte..byte + 1)).unwrap().is_empty());
            assert_eq!(callback_writers() - before, 0);
        }
        let output = p.feed(input.slice(input.len() - 1..)).unwrap();
        let expected = match format {
            GrpcJsonStreamFormat::Ndjson => b"{\"type\":\"message\",\"data\":{\"number\":3}}\n".as_slice(),
            GrpcJsonStreamFormat::Sse => b"event: rpc_message\ndata: {\"number\":3}\n\n".as_slice(),
        };
        assert_eq!(output, expected);
        assert_eq!(callback_writers() - before, 1);
        let mut coalesced = frame(&[]).to_vec();
        coalesced.extend_from_slice(&frame(&[8, 3]));
        let output = p.feed(Bytes::from(coalesced)).unwrap();
        let empty = match format {
            GrpcJsonStreamFormat::Ndjson => b"{\"type\":\"message\",\"data\":{}}\n".as_slice(),
            GrpcJsonStreamFormat::Sse => b"event: rpc_message\ndata: {}\n\n".as_slice(),
        };
        assert_eq!(output, [empty, expected].concat());
        assert_eq!(callback_writers() - before, 2);
        assert!(p.feed(Bytes::new()).unwrap().is_empty());
        assert_eq!(callback_writers() - before, 2);
    }
}

#[test]
fn callback_writer_preserves_completed_record_work_bound() {
    for format in [GrpcJsonStreamFormat::Ndjson, GrpcJsonStreamFormat::Sse] {
        let expected = match format {
            GrpcJsonStreamFormat::Ndjson => b"{\"type\":\"message\",\"data\":{}}\n".as_slice(),
            GrpcJsonStreamFormat::Sse => b"event: rpc_message\ndata: {}\n\n".as_slice(),
        };
        for records in [256, 257] {
            let mut p = processor(true, format);
            let before = callback_writers();
            let output = p.feed(Bytes::from(frame(&[]).repeat(records)));
            if records == 256 {
                assert_eq!(output.unwrap(), expected.repeat(records));
            } else {
                assert!(output.is_err());
            }
            assert_eq!(callback_writers() - before, 1);
        }
    }
}

fn string_stream_processor(format: GrpcJsonStreamFormat) -> GrpcResponseProcessor {
    use prost::Message;
    // Inferred descriptor types avoid adding a direct prost-types dependency.
    // Reserialization omits custom HTTP options; auto mapping supplies this fixture's binding.
    #[derive(prost::Message)]
    struct DescriptorSet {
        #[prost(bytes = "vec", repeated, tag = "1")]
        file: Vec<Vec<u8>>,
    }
    let source = processor(false, format);
    let mut changed = false;
    let file = source
        .plan
        .binding
        .method
        .parent_pool()
        .file_descriptor_protos()
        .map(|file| {
            let mut file = file.clone();
            for service in &mut file.service {
                for method in &mut service.method {
                    if file.package.as_deref() == Some("test")
                        && service.name.as_deref() == Some("TestService")
                        && method.name.as_deref() == Some("SayHello")
                    {
                        method.server_streaming = Some(true);
                        changed = true;
                    }
                }
            }
            file.encode_to_vec()
        })
        .collect();
    assert!(changed);
    let descriptor = DescriptorSet { file }.encode_to_vec();
    let mut config: GrpcJsonTranscoderConfig = serde_json::from_value(serde_json::json!({
        "descriptor": {"type": "inline", "base64": STANDARD.encode(descriptor)},
        "services": ["test.TestService"],
        "autoMapping": true
    }))
    .unwrap();
    config.stream_format = format;
    processor_from_config(config, "POST", "/test.TestService/SayHello")
}

#[test]
fn callback_writer_preserves_aggregate_and_message_json_caps() {
    use prost::Message;
    #[derive(prost::Message)]
    struct HelloResponse {
        #[prost(string, tag = "1")]
        message: String,
    }
    for format in [GrpcJsonStreamFormat::Ndjson, GrpcJsonStreamFormat::Sse] {
        let mut p = string_stream_processor(format);
        let payload = HelloResponse {
            message: "\0".repeat(100 * 1024),
        }
        .encode_to_vec();
        let expected = p
            .plan
            .binding
            .decode(&payload, p.plan.compiled.limits, Some(format))
            .unwrap();
        assert!(expected.len() > 512 * 1024);
        assert!(expected.len() <= 1024 * 1024);
        let before = callback_writers();
        assert_eq!(p.feed(frame(&payload)).unwrap(), expected);
        assert_eq!(callback_writers() - before, 1);
        let mut coalesced = frame(&payload).to_vec();
        coalesced.extend_from_slice(&frame(&payload));
        assert!(coalesced.len() < 512 * 1024);
        assert!(p.feed(Bytes::from(coalesced)).is_err());
        assert_eq!(callback_writers() - before, 2);

        let mut p = string_stream_processor(format);
        let payload = HelloResponse {
            message: "\0".repeat(180 * 1024),
        }
        .encode_to_vec();
        assert!(payload.len() < 512 * 1024);
        assert!(p
            .plan
            .binding
            .decode(&payload, p.plan.compiled.limits, Some(format))
            .is_err());
        let before = callback_writers();
        assert!(p.feed(frame(&payload)).is_err());
        assert_eq!(callback_writers() - before, 0);
    }
}
