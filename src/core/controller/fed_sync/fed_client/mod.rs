//! Federation client: connects to edgion-center and maintains a persistent gRPC stream.
//!
//! Lifecycle:
//! 1. Connect to center address
//! 2. Open bidirectional stream (Sync RPC)
//! 3. Send RegisterRequest as first message
//! 4. Loop: handle CenterMessage (Ping, ListRequest, CommandRequest, RegisterAck)
//! 5. On error: exponential backoff, reconnect

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tonic::transport::Channel;

use crate::core::common::config::CenterClientConfig;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload, controller_message::Payload as CtrlPayload,
    federation_sync_client::FederationSyncClient, CenterMessage, CommandResponse, ControllerMessage, ListResponse,
    Pong, RegisterRequest,
};
use crate::core::controller::conf_mgr::conf_center::traits::CenterApi;
use crate::core::controller::fed_sync::resource_collector;

/// Exponential backoff state for reconnection attempts.
pub struct ReconnectBackoff {
    current: u64,
}

impl ReconnectBackoff {
    pub fn new() -> Self {
        Self { current: 1 }
    }

    pub fn current_secs(&self) -> u64 {
        self.current
    }

    /// Advance to next backoff value (doubles, capped at 60s).
    pub fn next(&mut self) {
        self.current = (self.current * 2).min(60);
    }

    pub fn reset(&mut self) {
        self.current = 1;
    }
}

/// Runs the federation client loop. Should be spawned as a background task.
/// Exits only when `shutdown` is triggered.
pub async fn run(
    config: CenterClientConfig,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: Vec<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    if let Err(e) = config.security.validate() {
        tracing::error!(
            component = "fed_client",
            error = %e,
            "Invalid federation security config, federation client will not start"
        );
        return;
    }

    let mut backoff = ReconnectBackoff::new();

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown signal received, exiting");
                return;
            }
            result = connect_and_run(&config, conf_center.clone(), &supported_kinds) => {
                match result {
                    Ok(()) => {
                        tracing::info!(component = "fed_client", "Stream ended cleanly, reconnecting");
                        backoff.reset();
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = "fed_client",
                            error = %e,
                            retry_secs = backoff.current_secs(),
                            "Connection failed, will retry"
                        );
                    }
                }
            }
        }

        let wait = Duration::from_secs(backoff.current_secs());
        backoff.next();

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown during backoff, exiting");
                return;
            }
        }
    }
}

async fn connect_and_run(
    config: &CenterClientConfig,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: &[String],
) -> Result<()> {
    let tls = config.security.resolve_active_tls();
    let use_tls = tls.is_some_and(|t| !t.skip_tls);

    // Strip existing scheme and re-add based on TLS setting
    let addr = config
        .address
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let uri = if use_tls {
        format!("https://{}", addr)
    } else {
        format!("http://{}", addr)
    };

    let endpoint = Channel::from_shared(uri)?;
    let endpoint = if let Some(tls_cfg) = tls {
        if tls_cfg.skip_tls {
            tracing::error!(
                component = "fed_client",
                "security skip_tls=true: connecting WITHOUT TLS. INSECURE, emergency use only!"
            );
            endpoint
        } else {
            let client_tls = crate::core::common::grpc_tls::load_client_tls(tls_cfg).await?;
            endpoint
                .tls_config(client_tls)
                .map_err(|e| anyhow::anyhow!("Failed to configure federation client TLS: {}", e))?
        }
    } else {
        endpoint
    };

    let channel = endpoint.connect().await?;

    let mut client = FederationSyncClient::new(channel);

    let (tx, rx) = tokio::sync::mpsc::channel::<ControllerMessage>(32);
    let tx = Arc::new(tx);

    // Send RegisterRequest as first message
    let register = ControllerMessage {
        payload: Some(CtrlPayload::Register(RegisterRequest {
            controller_id: config.controller_id(),
            cluster: config.cluster.clone(),
            env: config.env.clone(),
            tag: config.tag.clone(),
            supported_kinds: supported_kinds
                .iter()
                .filter(|k| !resource_collector::is_fed_no_sync_kind(k))
                .cloned()
                .collect(),
        })),
    };
    tx.send(register).await?;

    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut inbound = client.sync(outbound).await?.into_inner();

    tracing::info!(
        component = "fed_client",
        controller_id = %config.controller_id(),
        "Connected and registered with center"
    );

    loop {
        tokio::select! {
            msg = inbound.message() => {
                match msg? {
                    None => {
                        tracing::info!(component = "fed_client", "Center closed stream");
                        return Ok(());
                    }
                    Some(CenterMessage { payload: Some(payload) }) => {
                        handle_center_message(
                            payload,
                            tx.clone(),
                            conf_center.clone(),
                            supported_kinds,
                        ).await?;
                    }
                    Some(_) => {} // empty payload, ignore
                }
            }
        }
    }
}

async fn handle_center_message(
    payload: CenterPayload,
    tx: Arc<tokio::sync::mpsc::Sender<ControllerMessage>>,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: &[String],
) -> Result<()> {
    match payload {
        CenterPayload::RegisterAck(ack) => {
            tracing::info!(
                component = "fed_client",
                session_id = %ack.session_id,
                "Registered with center"
            );
        }

        CenterPayload::Ping(ping) => {
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::Pong(Pong {
                    timestamp: ping.timestamp,
                })),
            })
            .await?;
        }

        CenterPayload::ListRequest(req) => {
            tracing::debug!(
                component = "fed_client",
                request_id = %req.request_id,
                "ListRequest received"
            );
            let keys = match resource_collector::collect_resource_keys(conf_center.as_ref(), supported_kinds).await {
                Ok(keys) => keys,
                Err(e) => {
                    tracing::error!(component = "fed_client", error = %e, "collect_resource_keys failed");
                    vec![]
                }
            };
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::ListResponse(ListResponse {
                    request_id: req.request_id,
                    keys,
                })),
            })
            .await?;
        }

        CenterPayload::Command(cmd) => {
            tracing::info!(
                component = "fed_client",
                request_id = %cmd.request_id,
                "CommandRequest received"
            );
            // Command execution stubbed — respond with not-implemented
            let _ = conf_center; // suppress unused warning
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::CommandResponse(CommandResponse {
                    request_id: cmd.request_id,
                    success: false,
                    message: "Command execution not yet implemented".to_string(),
                })),
            })
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_caps_at_60s() {
        let mut backoff = ReconnectBackoff::new();
        for _ in 0..20 {
            backoff.next();
        }
        assert_eq!(backoff.current_secs(), 60);
    }

    #[test]
    fn test_backoff_resets() {
        let mut backoff = ReconnectBackoff::new();
        backoff.next(); // 2
        backoff.next(); // 4
        backoff.reset();
        assert_eq!(backoff.current_secs(), 1);
    }
}
