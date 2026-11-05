//! Bounded federation HTTP exchange. No source paths or payloads enter diagnostics.
use super::{ALLOWED_PROXY_HEADERS, ALLOWED_PROXY_PATH_PREFIXES, MAX_PROXY_REQUEST_BODY};
use crate::fed_sync::proto::{controller_message::Payload, ControllerMessage, HttpProxyRequest, HttpProxyResponse};
use std::{collections::HashMap, time::Duration};
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tower::ServiceExt;

pub(super) const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;
pub(super) const PROXY_TIMEOUT: Duration = Duration::from_secs(12);

pub(super) fn error(request_id: String, status_code: u32, message: &'static str) -> HttpProxyResponse {
    let request_id = if request_id.len() <= 128 {
        request_id
    } else {
        String::new()
    };
    HttpProxyResponse {
        request_id,
        status_code,
        headers: HashMap::new(),
        body: message.as_bytes().to_vec(),
    }
}

pub(super) async fn send(
    tx: &tokio::sync::mpsc::Sender<ControllerMessage>,
    response: HttpProxyResponse,
    deadline: Instant,
) {
    let message = ControllerMessage {
        payload: Some(Payload::HttpProxyResponse(response)),
    };
    // An expired exchange may still report its fixed timeout without waiting for capacity.
    if Instant::now() >= deadline {
        let _ = tx.try_send(message);
    } else {
        let _ = tokio::time::timeout_at(deadline, tx.send(message)).await;
    }
}

pub(super) async fn execute(router: axum::Router, req: HttpProxyRequest) -> HttpProxyResponse {
    let id = req.request_id;
    if id.len() > 128 {
        return error(String::new(), 400, "Invalid proxy request identifier");
    }
    if req.body.len() > MAX_PROXY_REQUEST_BODY
        || req.path.len() > 16 * 1024
        || req.method.len() > 32
        || req.headers.len() > 64
        || req.headers.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > MAX_HEADER_BYTES
    {
        return error(id, 413, "Proxy request exceeds envelope limit");
    }
    let Ok(method) = req.method.parse::<http::Method>() else {
        return error(id, 400, "Invalid proxy method");
    };
    let path = req.path.split(['?', '#']).next().unwrap_or("");
    if !ALLOWED_PROXY_PATH_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return error(id, 404, "Proxy path not allowed");
    }
    let mut builder = http::Request::builder().method(method).uri(&req.path);
    for (key, value) in req.headers {
        if ALLOWED_PROXY_HEADERS
            .iter()
            .any(|allowed| key.eq_ignore_ascii_case(allowed))
        {
            builder = builder.header(key, value);
        }
    }
    let Ok(request) = builder.body(axum::body::Body::from(req.body)) else {
        return error(id, 400, "Invalid proxy request");
    };
    let response = match router.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let status_code = u32::from(response.status().as_u16());
    if !(200..=599).contains(&status_code) {
        return error(id, 502, "Invalid proxy response status");
    }
    if response.headers().len() > 64
        || response
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len())
            .sum::<usize>()
            > MAX_HEADER_BYTES
    {
        return error(id, 413, "Proxy response exceeds envelope limit");
    }
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let mut stream = response.into_body().into_data_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return error(id, 502, "Failed to read proxy response");
        };
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return error(id, 413, "Proxy response exceeds envelope limit");
        }
        body.extend_from_slice(&chunk);
    }
    HttpProxyResponse {
        request_id: id,
        status_code,
        headers,
        body,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/fed_sync/fed_client/proxy_test.rs"]
mod tests;
