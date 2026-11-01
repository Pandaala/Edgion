//! Test-only Controller Admin API update helpers.
//!
//! Kubernetes status writes can advance a resource's `resourceVersion` during
//! the Controller's internal GET -> replace window. The Admin API deliberately
//! surfaces that backend 409 even for an unconditional update, so integration
//! clients retry only that narrow, idempotent case here instead of changing the
//! product API contract.

use anyhow::{Context, Result};
use reqwest::{Response, StatusCode};
use std::future::Future;
use std::time::Duration;

const CONFLICT_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
];
const CONFLICT_RETRY_DEADLINE: Duration = Duration::from_secs(5);

/// Whether the YAML document requests optimistic concurrency explicitly.
///
/// A string `metadata.resourceVersion` is the Admin API's compatibility CAS
/// token. Those requests must surface 409 and are never safe to replay as an
/// unconditional test update.
pub fn has_explicit_resource_version(document: &yaml_serde::Value) -> bool {
    document
        .get("metadata")
        .and_then(|metadata| metadata.get("resourceVersion"))
        .and_then(yaml_serde::Value::as_str)
        .is_some()
}

/// Send one unconditional PUT, retrying only a Kubernetes backend 409.
///
/// The first request plus three retries are bounded by one five-second
/// deadline. Authentication, validation, transport, and explicit-CAS failures
/// remain fail-closed at their call sites.
pub async fn put_unconditional_with_conflict_retry<F, Fut>(mut send: F) -> Result<Response>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Response>>,
{
    tokio::time::timeout(CONFLICT_RETRY_DEADLINE, async {
        for (attempt, retry_delay) in CONFLICT_RETRY_DELAYS.iter().enumerate() {
            let response = send().await?;
            if response.status() != StatusCode::CONFLICT {
                return Ok(response);
            }
            tracing::warn!(
                attempt = attempt + 1,
                max_attempts = CONFLICT_RETRY_DELAYS.len() + 1,
                retry_delay_ms = retry_delay.as_millis() as u64,
                "Test Admin API PUT conflicted with a backend update; retrying"
            );
            tokio::time::sleep(*retry_delay).await;
        }
        send().await
    })
    .await
    .context("Test Admin API PUT conflict retry exceeded the 5-second deadline")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn detects_only_string_resource_version_as_explicit_cas() {
        let absent: yaml_serde::Value = yaml_serde::from_str("metadata: {name: route}").unwrap();
        let explicit: yaml_serde::Value =
            yaml_serde::from_str("metadata: {name: route, resourceVersion: '42'}").unwrap();
        let null_value: yaml_serde::Value =
            yaml_serde::from_str("metadata: {name: route, resourceVersion: null}").unwrap();

        assert!(!has_explicit_resource_version(&absent));
        assert!(has_explicit_resource_version(&explicit));
        assert!(!has_explicit_resource_version(&null_value));
    }

    #[tokio::test]
    async fn retries_conflicts_until_success() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/resource"))
            .respond_with(ResponseTemplate::new(409))
            .with_priority(1)
            .up_to_n_times(3)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/resource"))
            .respond_with(ResponseTemplate::new(200))
            .with_priority(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/resource", server.uri());
        let attempts = Arc::new(AtomicUsize::new(0));
        let response = put_unconditional_with_conflict_retry(|| {
            attempts.fetch_add(1, Ordering::Relaxed);
            let request = client.put(&url);
            async move { request.send().await.map_err(anyhow::Error::from) }
        })
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(attempts.load(Ordering::Relaxed), 4);
    }

    #[tokio::test]
    async fn does_not_retry_non_conflict_response() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/resource"))
            .respond_with(ResponseTemplate::new(422))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/resource", server.uri());
        let attempts = Arc::new(AtomicUsize::new(0));
        let response = put_unconditional_with_conflict_retry(|| {
            attempts.fetch_add(1, Ordering::Relaxed);
            let request = client.put(&url);
            async move { request.send().await.map_err(anyhow::Error::from) }
        })
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }
}
