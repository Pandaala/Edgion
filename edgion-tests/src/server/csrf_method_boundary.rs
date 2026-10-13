//! Fixed, content-free origin observations for the CSRF composition suite.

use axum::{extract::Request, response::IntoResponse, Json};
use parking_lot::Mutex;
use serde::Serialize;

#[derive(Default, Clone, Copy, Serialize)]
struct Observation {
    calls: u64,
    reads: u64,
    writes: u64,
}

static OBSERVATION: Mutex<Observation> = Mutex::new(Observation {
    calls: 0,
    reads: 0,
    writes: 0,
});

pub async fn observation() -> impl IntoResponse {
    Json(*OBSERVATION.lock())
}

fn authenticated(request: &Request) -> bool {
    request.headers().get_all("cookie").iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(';')
                .any(|cookie| cookie.trim() == "fixture-session=authenticated")
        })
    })
}

fn reply(method: &str, path: &str, write: bool) -> axum::response::Response {
    let mut observation = OBSERVATION.lock();
    if write {
        observation.writes += 1;
    } else {
        observation.reads += 1;
    }
    Json(serde_json::json!({"method": method, "path": path, "effect": if write { "write" } else { "read" }}))
        .into_response()
}

pub async fn action(request: Request) -> axum::response::Response {
    OBSERVATION.lock().calls += 1;
    let method = request.method().as_str();
    if method == "POST" && !authenticated(&request) {
        return (http::StatusCode::UNAUTHORIZED, "fixture authentication required").into_response();
    }
    reply(method, request.uri().path(), method == "POST")
}

pub async fn graphql(request: Request) -> axum::response::Response {
    OBSERVATION.lock().calls += 1;
    let authenticated = authenticated(&request);
    let body = match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        axum::body::to_bytes(request.into_body(), 4096),
    )
    .await
    {
        Ok(Ok(body)) => body,
        _ => return (http::StatusCode::BAD_REQUEST, "invalid fixture body").into_response(),
    };
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (http::StatusCode::BAD_REQUEST, "invalid fixture JSON").into_response();
    };
    let write = match document["query"].as_str() {
        Some("query Read{read}") => false,
        Some("mutation Write{write}") => true,
        _ => return (http::StatusCode::BAD_REQUEST, "unknown fixture operation").into_response(),
    };
    if write && !authenticated {
        return (http::StatusCode::UNAUTHORIZED, "fixture authentication required").into_response();
    }
    reply("POST", "/csrf-method-boundary/graphql", write)
}
