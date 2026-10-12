//! Local native-AI measurement fixture. Generates lazily, without pacing or payload retention.
use super::{AiLongStreamObservation, AI_LONG_STREAM_OBSERVATIONS};
use axum::{
    body::{Body, Bytes},
    extract::{Query, Request},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{convert::Infallible, sync::LazyLock, time::Duration};
use tokio::sync::{mpsc, Semaphore};

const MAX_OUTPUT_BYTES: usize = 1024 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
static STREAMS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(64));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Options {
    #[serde(default = "default_payload")]
    payload: usize,
    #[serde(default = "default_frames")]
    frames: usize,
}
fn default_payload() -> usize {
    1024
}
fn default_frames() -> usize {
    8
}

fn update(trace: &Option<String>, f: impl FnOnce(&mut AiLongStreamObservation)) {
    if let Some(trace) = trace {
        if let Some(state) = AI_LONG_STREAM_OBSERVATIONS.lock().get_mut(trace) {
            f(state);
        }
    }
}

pub(super) async fn handler(Query(options): Query<Options>, request: Request) -> Response {
    if options.payload == 0 || options.payload > MAX_PAYLOAD_BYTES || options.frames == 0 {
        return (StatusCode::BAD_REQUEST, "Invalid performance bounds").into_response();
    }
    // Bound wire bytes, including each frame's structural overhead and terminal frames.
    if options.frames > (MAX_OUTPUT_BYTES - 4096) / (options.payload + 1024) {
        return (StatusCode::BAD_REQUEST, "Performance output cap exceeded").into_response();
    }
    let trace = if request.headers().get("x-ai-performance-observe").is_some() {
        let Some(trace) = request.headers().get("x-trace-id").and_then(|v| v.to_str().ok()) else {
            return (StatusCode::BAD_REQUEST, "Missing performance trace").into_response();
        };
        if trace.len() != 32 || !trace.bytes().all(|v| v.is_ascii_hexdigit()) {
            return (StatusCode::BAD_REQUEST, "Invalid performance trace").into_response();
        }
        Some(trace.to_owned())
    } else {
        None
    };
    let Ok(body) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "Performance request cap exceeded").into_response();
    };
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "Invalid performance request").into_response();
    };
    let streaming = document
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let content = "X".repeat(options.payload);
    if !streaming {
        let json = format!(
            r#"{{"id":"perf","object":"chat.completion","created":0,"model":"perf-model","choices":[{{"index":0,"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}}}"#
        );
        return ([(header::CONTENT_TYPE, "application/json")], json).into_response();
    }
    let Ok(permit) = STREAMS.try_acquire() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Performance stream admission exhausted",
        )
            .into_response();
    };
    if let Some(trace) = &trace {
        let mut observations = AI_LONG_STREAM_OBSERVATIONS.lock();
        if observations.len() >= 128 || observations.contains_key(trace) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Performance observation admission exhausted",
            )
                .into_response();
        }
        observations.insert(trace.clone(), AiLongStreamObservation::default());
    }
    let frame = Bytes::from(format!("data: {{\"id\":\"perf\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"perf-model\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\n"));
    let terminal = Bytes::from_static(b"data: {\"id\":\"perf\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"perf-model\",\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\ndata: [DONE]\n\n");
    let (tx, rx) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    tokio::spawn(async move {
        let _permit = permit;
        let deadline = tokio::time::sleep(Duration::from_secs(45));
        tokio::pin!(deadline);
        for index in 0..=options.frames {
            let bytes = if index == options.frames {
                terminal.clone()
            } else {
                frame.clone()
            };
            let len = bytes.len() as u64;
            update(&trace, |s| s.pending_send = true);
            let sent = tokio::select! {
                biased;
                _ = tx.closed() => { update(&trace, |s| { s.upstream_cancelled = true; s.pending_send = false; }); return; }
                _ = &mut deadline => { update(&trace, |s| { s.deadline_expired = true; s.pending_send = false; }); return; }
                result = tx.send(Ok(bytes)) => result.is_ok(),
            };
            if !sent {
                update(&trace, |s| {
                    s.upstream_cancelled = true;
                    s.pending_send = false;
                });
                return;
            }
            update(&trace, |s| {
                s.chunks_queued += 1;
                s.bytes_queued += len;
                s.pending_send = false;
            });
        }
        update(&trace, |s| s.provider_completed = true);
    });
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)),
    )
        .into_response()
}

pub(super) async fn delete_observation(axum::extract::Path(trace): axum::extract::Path<String>) -> impl IntoResponse {
    AI_LONG_STREAM_OBSERVATIONS.lock().remove(&trace);
    StatusCode::NO_CONTENT
}
