//! Bounded JSON reads for native source lists and their shared version discovery.
use crate::conf_mgr::conf_center::{source_query as query, traits::ConfWriterError};
use futures::StreamExt;
use serde::de::DeserializeOwned;

pub(super) async fn json<T: DeserializeOwned>(
    client: &kube::Client,
    request: http::Request<Vec<u8>>,
) -> Result<T, ConfWriterError> {
    tokio::time::timeout(query::SOURCE_TIMEOUT, async {
        let response = client
            .send(request.map(kube::client::Body::from))
            .await
            .map_err(|_| query::source_error())?;
        // Never collect upstream error bodies: they can echo credentials or tokens.
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(ConfWriterError::PermissionDenied(
                    "Kubernetes source access denied".into(),
                ))
            }
            404 => {
                return Err(ConfWriterError::NotFound(
                    "Kubernetes source collection not found".into(),
                ))
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
        let mut stream = axum::body::Body::new(response.into_body()).into_data_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| query::source_error())?;
            if chunk.len() > query::MAX_SOURCE_BYTES.saturating_sub(bytes.len()) {
                return Err(query::budget_error());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| query::source_error())
    })
    .await
    .map_err(|_| ConfWriterError::SourceTimeout)?
}
