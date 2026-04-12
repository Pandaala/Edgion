//! ProxyForwarder: forwards HTTP requests to a specific controller via the gRPC bidirectional stream.

use axum::http::StatusCode;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload, CenterMessage, HttpProxyRequest, HttpProxyResponse,
};

pub type PendingProxyMap = Arc<Mutex<HashMap<String, oneshot::Sender<HttpProxyResponse>>>>;

pub struct ProxyForwarder {
    registry: ControllerRegistry,
    pending: PendingProxyMap,
    timeout: Duration,
}

impl ProxyForwarder {
    pub fn new(registry: ControllerRegistry, pending: PendingProxyMap, timeout_secs: u64) -> Self {
        Self {
            registry,
            pending,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    pub async fn forward(
        &self,
        controller_id: &str,
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpProxyResponse, (StatusCode, String)> {
        let session = self.registry.get_session(controller_id).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Controller {} not found or offline", controller_id),
            )
        })?;

        if session.offline_since.is_some() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Controller {} is offline", controller_id),
            ));
        }

        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel::<HttpProxyResponse>();
        self.pending.lock().insert(request_id.clone(), tx);

        let msg = CenterMessage {
            payload: Some(CenterPayload::HttpProxy(HttpProxyRequest {
                request_id: request_id.clone(),
                method,
                path,
                headers,
                body,
            })),
        };

        session.stream_tx.send(msg).await.map_err(|_| {
            self.pending.lock().remove(&request_id);
            (
                StatusCode::BAD_GATEWAY,
                "Failed to send proxy request: stream closed".to_string(),
            )
        })?;

        tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| {
                self.pending.lock().remove(&request_id);
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("Proxy request timed out after {}s", self.timeout.as_secs()),
                )
            })?
            .map_err(|_| {
                self.pending.lock().remove(&request_id);
                (StatusCode::BAD_GATEWAY, "Proxy response channel dropped".to_string())
            })
    }
}
