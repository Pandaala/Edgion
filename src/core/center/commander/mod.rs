//! CommandDispatcher: sends CommandRequest to a specific controller and awaits response.

use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::PendingCommandMap;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload, CenterMessage, CommandRequest, CommandResponse,
};

pub struct Commander {
    registry: ControllerRegistry,
    pending: PendingCommandMap,
    timeout: Duration,
}

impl Commander {
    pub fn new(registry: ControllerRegistry, pending: PendingCommandMap, timeout_secs: u64) -> Self {
        Self {
            registry,
            pending,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    pub async fn send_command(
        &self,
        controller_id: &str,
        command: crate::core::common::fed_sync::proto::command_request::Command,
    ) -> Result<CommandResponse> {
        let session = self
            .registry
            .get_session(controller_id)
            .ok_or_else(|| anyhow!("Controller {} not found or offline", controller_id))?;

        if session.offline_since.is_some() {
            return Err(anyhow!("Controller {} is offline", controller_id));
        }

        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel::<CommandResponse>();
        self.pending.lock().insert(request_id.clone(), tx);

        let msg = CenterMessage {
            payload: Some(CenterPayload::Command(CommandRequest {
                request_id: request_id.clone(),
                command: Some(command),
            })),
        };

        session.stream_tx.send(msg).await.map_err(|_| {
            self.pending.lock().remove(&request_id);
            anyhow!("Failed to send command: stream closed")
        })?;

        tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| {
                self.pending.lock().remove(&request_id);
                anyhow!("Command timed out after {}s", self.timeout.as_secs())
            })?
            .map_err(|_| {
                self.pending.lock().remove(&request_id);
                anyhow!("Command response channel dropped")
            })
    }
}
