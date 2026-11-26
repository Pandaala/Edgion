//! Bounded JSON reads for native source lists and their shared version discovery.
use crate::conf_mgr::conf_center::{source_query as query, traits::ConfWriterError};
use futures::StreamExt;
use kube::core::ApiResource;
use serde::de::DeserializeOwned;
use serde::Deserialize;

#[derive(Deserialize)]
struct MissingStatus {
    kind: String,
    status: String,
    code: u16,
    reason: String,
    details: MissingDetails,
}

#[derive(Deserialize)]
struct MissingDetails {
    name: String,
    #[serde(default)]
    group: String,
    kind: String,
}

pub(super) async fn named_json(
    client: &kube::Client,
    request: http::Request<Vec<u8>>,
    resource: &ApiResource,
    name: &str,
) -> Result<serde_json::Value, ConfWriterError> {
    request_json(client, request, Some((resource, name))).await
}

pub(super) async fn json<T: DeserializeOwned>(
    client: &kube::Client,
    request: http::Request<Vec<u8>>,
) -> Result<T, ConfWriterError> {
    request_json(client, request, None).await
}

async fn request_json<T: DeserializeOwned>(
    client: &kube::Client,
    request: http::Request<Vec<u8>>,
    named: Option<(&ApiResource, &str)>,
) -> Result<T, ConfWriterError> {
    tokio::time::timeout(query::SOURCE_TIMEOUT, async {
        let response = client
            .send(request.map(kube::client::Body::from))
            .await
            .map_err(|_| query::source_error())?;
        // Only the named GET's 404 needs structured evidence. Never propagate upstream error text.
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(ConfWriterError::PermissionDenied(
                    "Kubernetes source access denied".into(),
                ))
            }
            404 => {
                if let Some((resource, name)) = named {
                    let status: MissingStatus = read_json(response.into_body(), 8192).await?;
                    if status.kind != "Status"
                        || status.status != "Failure"
                        || status.code != 404
                        || status.reason != "NotFound"
                        || status.details.name != name
                        || status.details.group != resource.group
                        || status.details.kind != resource.plural
                    {
                        return Err(query::source_error());
                    }
                    return Err(ConfWriterError::NotFound("Source object not found".into()));
                }
                return Err(ConfWriterError::NotFound(
                    "Kubernetes source collection not found".into(),
                ));
            }
            409 => return Err(ConfWriterError::Conflict("Source changed since preview".into())),
            422 => {
                return Err(ConfWriterError::ValidationError(
                    "Kubernetes rejected the source update".into(),
                ))
            }
            410 => return Err(query::stale()),
            429 => return Err(ConfWriterError::SourceBusy),
            504 => return Err(ConfWriterError::SourceTimeout),
            _ => return Err(query::source_error()),
        }
        read_json(response.into_body(), query::MAX_SOURCE_BYTES).await
    })
    .await
    .map_err(|_| ConfWriterError::SourceTimeout)?
}

async fn read_json<T: DeserializeOwned>(body: kube::client::Body, limit: usize) -> Result<T, ConfWriterError> {
    let mut stream = axum::body::Body::new(body).into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| query::source_error())?;
        if chunk.len() > limit - bytes.len() {
            return Err(query::budget_error());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| query::source_error())
}
