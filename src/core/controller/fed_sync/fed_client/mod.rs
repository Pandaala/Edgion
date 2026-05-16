//! Federation client: connects to edgion-center and maintains a persistent gRPC stream.
//!
//! Lifecycle:
//! 1. Connect to center address
//! 2. Open bidirectional stream (Sync RPC)
//! 3. Send RegisterRequest as first message
//! 4. Loop: handle CenterMessage (Ping, CommandRequest, RegisterAck, WatchRequest)
//! 5. On error: exponential backoff, reconnect

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Semaphore};
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

use crate::core::common::config::CenterClientConfig;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload, controller_message::Payload as CtrlPayload,
    federation_sync_client::FederationSyncClient, CenterMessage, CommandResponse, ControllerMessage,
    FedWatchEventResponse, FedWatchListResponse, HttpProxyResponse, Pong, RegisterRequest,
};
use crate::core::common::observe::fed_metrics;
use crate::core::controller::conf_mgr::ConfMgr;

/// Minimum connection duration before backoff is reset.
/// Connections shorter than this are considered unstable (e.g., Center crash-loop).
const MIN_STABLE_CONNECTION_SECS: u64 = 30;

/// Maximum concurrent HttpProxy tasks forwarded from Center.
const MAX_CONCURRENT_PROXY: usize = 16;

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
    supported_kinds: Vec<String>,
    mut shutdown: watch::Receiver<bool>,
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
) {
    if let Err(e) = config.security.validate() {
        tracing::error!(
            component = "fed_client",
            error = %e,
            "Invalid federation security config, federation client will not start"
        );
        return;
    }

    // Wait for ConfigSyncServer to be ready before connecting to Center.
    // This prevents the tight NotReady error loop where Center sends
    // FedWatchRequest before the Controller's caches are initialized.
    tracing::info!(
        component = "fed_client",
        "Waiting for ConfigSyncServer to be ready before connecting to Center"
    );
    let ready_wait_start = std::time::Instant::now();
    loop {
        if conf_mgr.is_ready() {
            let elapsed = std::time::Instant::now()
                .saturating_duration_since(ready_wait_start)
                .as_secs_f64();
            fed_metrics::record_ready_gate_wait(elapsed);
            tracing::info!(
                component = "fed_client",
                elapsed_secs = elapsed,
                "ConfigSyncServer is ready, proceeding to connect"
            );
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown during ready-wait, exiting");
                return;
            }
        }
    }

    let mut backoff = ReconnectBackoff::new();

    loop {
        let connect_start = std::time::Instant::now();
        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown signal received, exiting");
                return;
            }
            result = connect_and_run(&config, &supported_kinds, admin_router.clone(), conf_mgr.clone()) => {
                match result {
                    Ok(()) => {
                        // Only reset backoff if connection was alive long enough.
                        // Prevents rapid reconnection when Center crash-loops
                        // (connects briefly then dies), which would cause repeated
                        // TLS handshakes + full resource serialization.
                        if std::time::Instant::now().saturating_duration_since(connect_start) > Duration::from_secs(MIN_STABLE_CONNECTION_SECS) {
                            backoff.reset();
                        }
                        tracing::info!(component = "fed_client", "Stream ended cleanly, reconnecting");
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
    supported_kinds: &[String],
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
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
            crate::core::common::observe::system_metrics::mark_system_error(
                crate::core::common::observe::system_metrics::components::FED_SYNC_CLIENT,
                crate::core::common::observe::system_metrics::reasons::SKIP_TLS,
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
            supported_kinds: supported_kinds.to_vec(),
        })),
    };
    tx.send(register).await?;

    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut inbound = client.sync(outbound).await?.into_inner();

    fed_metrics::record_connection_event(
        fed_metrics::labels::role::CONTROLLER,
        fed_metrics::labels::event::CONNECTED,
    );
    fed_metrics::set_connections_active(fed_metrics::labels::role::CONTROLLER, 1);
    let session_started_at = std::time::Instant::now();

    tracing::info!(
        component = "fed_client",
        controller_id = %config.controller_id(),
        "Connected and registered with center"
    );

    // Tracks the cancellation token for the active watch forwarding task, if any.
    // A new WatchRequest cancels the previous one before starting a new task.
    let mut active_watch_cancel: Option<CancellationToken> = None;

    // Limit concurrent HttpProxy tasks to prevent Center from overwhelming Controller.
    let proxy_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PROXY));

    let result: Result<()> = loop {
        tokio::select! {
            msg = inbound.message() => {
                match msg {
                    Err(e) => break Err(e.into()),
                    Ok(None) => {
                        tracing::info!(component = "fed_client", "Center closed stream");
                        break Ok(());
                    }
                    Ok(Some(CenterMessage { payload: Some(payload) })) => {
                        if let Err(e) = handle_center_message(
                            payload,
                            tx.clone(),
                            admin_router.clone(),
                            conf_mgr.clone(),
                            &mut active_watch_cancel,
                            proxy_semaphore.clone(),
                        ).await {
                            break Err(e);
                        }
                    }
                    Ok(Some(_)) => {} // empty payload, ignore
                }
            }
        }
    };

    // Cleanup: cancel any active watch forwarding task on disconnect.
    // Without this, old forwarding tasks linger until the gRPC stream's
    // internal channels detect the disconnect asynchronously.
    if let Some(prev) = active_watch_cancel.take() {
        prev.cancel();
    }

    // Record session end metrics. This runs whether the loop broke with
    // Ok(()) (clean close) or Err(..) (stream error); both count as a
    // disconnect event on the Controller side.
    fed_metrics::record_connection_event(
        fed_metrics::labels::role::CONTROLLER,
        fed_metrics::labels::event::DISCONNECTED,
    );
    fed_metrics::record_connection_duration(
        fed_metrics::labels::role::CONTROLLER,
        std::time::Instant::now()
            .saturating_duration_since(session_started_at)
            .as_secs_f64(),
    );
    fed_metrics::set_connections_active(fed_metrics::labels::role::CONTROLLER, 0);

    result
}

async fn handle_center_message(
    payload: CenterPayload,
    tx: Arc<tokio::sync::mpsc::Sender<ControllerMessage>>,
    admin_router: axum::Router,
    conf_mgr: Arc<ConfMgr>,
    active_watch_cancel: &mut Option<CancellationToken>,
    proxy_semaphore: Arc<Semaphore>,
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

        CenterPayload::Command(cmd) => {
            tracing::info!(
                component = "fed_client",
                request_id = %cmd.request_id,
                "CommandRequest received"
            );
            use crate::core::common::fed_sync::proto::command_request::Command;
            let (success, message) = match cmd.command {
                Some(Command::Reload(_)) => {
                    use tower::ServiceExt;
                    match http::Request::builder()
                        .method(http::Method::POST)
                        .uri("/api/v1/reload")
                        .body(axum::body::Body::empty())
                    {
                        Ok(req) => match admin_router.clone().oneshot(req).await {
                            Ok(resp) if resp.status().is_success() => (true, "Reload requested".to_string()),
                            Ok(resp) => (false, format!("Reload returned {}", resp.status())),
                            Err(e) => (false, format!("Reload failed: {}", e)),
                        },
                        Err(e) => (false, format!("Failed to build reload request: {}", e)),
                    }
                }
                _ => (false, "Command not supported".to_string()),
            };
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::CommandResponse(CommandResponse {
                    request_id: cmd.request_id,
                    success,
                    message,
                })),
            })
            .await?;
        }

        CenterPayload::HttpProxy(proxy_req) => {
            tracing::debug!(
                component = "fed_client",
                request_id = %proxy_req.request_id,
                method = %proxy_req.method,
                path = %proxy_req.path,
                "HttpProxyRequest received"
            );
            // Limit concurrent proxy tasks to prevent Center from overwhelming Controller.
            let permit = match proxy_semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tracing::warn!(
                        component = "fed_client",
                        request_id = %proxy_req.request_id,
                        "HttpProxy rejected: too many concurrent requests"
                    );
                    let _ = tx
                        .send(ControllerMessage {
                            payload: Some(CtrlPayload::HttpProxyResponse(HttpProxyResponse {
                                request_id: proxy_req.request_id,
                                status_code: 429,
                                headers: Default::default(),
                                body: b"Too many concurrent proxy requests".to_vec(),
                            })),
                        })
                        .await;
                    return Ok(());
                }
            };
            let tx = tx.clone();
            // Spawn as a separate task so proxy requests don't block Ping/Pong handling
            tokio::spawn(async move {
                let _permit = permit; // held until task completes
                let request_id = proxy_req.request_id.clone();

                let method: http::Method = proxy_req.method.parse().unwrap_or(http::Method::GET);
                let mut builder = http::Request::builder().method(method).uri(&proxy_req.path);

                for (k, v) in &proxy_req.headers {
                    builder = builder.header(k.as_str(), v.as_str());
                }

                let request = match builder.body(axum::body::Body::from(proxy_req.body)) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            component = "fed_client",
                            request_id = %request_id,
                            error = %e,
                            "Failed to build proxy request"
                        );
                        let _ = tx
                            .send(ControllerMessage {
                                payload: Some(CtrlPayload::HttpProxyResponse(HttpProxyResponse {
                                    request_id,
                                    status_code: 400,
                                    headers: Default::default(),
                                    body: format!("Bad proxy request: {}", e).into_bytes(),
                                })),
                            })
                            .await;
                        return;
                    }
                };

                // Call the router directly (no network) using tower::ServiceExt::oneshot
                use tower::ServiceExt;
                match admin_router.oneshot(request).await {
                    Ok(response) => {
                        let status = response.status().as_u16() as u32;
                        let headers: std::collections::HashMap<String, String> = response
                            .headers()
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        let body_bytes = match axum::body::to_bytes(response.into_body(), 10 * 1024 * 1024).await {
                            Ok(b) => b.to_vec(),
                            Err(e) => {
                                tracing::warn!(
                                    component = "fed_client",
                                    request_id = %request_id,
                                    error = %e,
                                    "Failed to read proxy response body"
                                );
                                format!("Failed to read response body: {}", e).into_bytes()
                            }
                        };
                        tracing::debug!(
                            component = "fed_client",
                            request_id = %request_id,
                            status_code = status,
                            "HttpProxyResponse sent"
                        );
                        let _ = tx
                            .send(ControllerMessage {
                                payload: Some(CtrlPayload::HttpProxyResponse(HttpProxyResponse {
                                    request_id,
                                    status_code: status,
                                    headers,
                                    body: body_bytes,
                                })),
                            })
                            .await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = "fed_client",
                            request_id = %request_id,
                            error = %e,
                            "Internal proxy error"
                        );
                        let _ = tx
                            .send(ControllerMessage {
                                payload: Some(CtrlPayload::HttpProxyResponse(HttpProxyResponse {
                                    request_id,
                                    status_code: 502,
                                    headers: Default::default(),
                                    body: format!("Internal proxy error: {}", e).into_bytes(),
                                })),
                            })
                            .await;
                    }
                }
            });
        }

        CenterPayload::WatchRequest(req) => {
            tracing::debug!(
                component = "fed_client",
                request_id = %req.request_id,
                kind = %req.kind,
                from_version = req.from_version,
                "FedWatchRequest received"
            );

            // If ConfigSyncServer is not ready, return NotReady immediately.
            // The ready gate in run() ensures CSS is ready before first connection.
            // Center-side backoff (3s) handles transient unavailability during reload.
            // We must NOT block here — this would stall Ping/Pong and cause heartbeat timeout.
            let css = match conf_mgr.config_sync_server() {
                Some(css) => css,
                None => {
                    tracing::info!(
                        component = "fed_client",
                        request_id = %req.request_id,
                        "ConfigSyncServer not ready, returning NotReady (Center will retry with backoff)"
                    );
                    let err_msg = ControllerMessage {
                        payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                            request_id: req.request_id,
                            error: "NotReady".to_string(),
                            ..Default::default()
                        })),
                    };
                    let _ = tx.send(err_msg).await;
                    return Ok(());
                }
            };

            // Step 1: if from_version == 0, send list first
            let watch_from = if req.from_version == 0 {
                match css.list(&req.kind) {
                    Ok(list_data) => {
                        let list_msg = ControllerMessage {
                            payload: Some(CtrlPayload::WatchListResponse(FedWatchListResponse {
                                request_id: req.request_id.clone(),
                                data: list_data.data,
                                sync_version: list_data.sync_version,
                                server_id: list_data.server_id,
                            })),
                        };
                        if tx.send(list_msg).await.is_err() {
                            return Ok(());
                        }
                        list_data.sync_version
                    }
                    Err(e) => {
                        let err_msg = ControllerMessage {
                            payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                                request_id: req.request_id,
                                error: e,
                                ..Default::default()
                            })),
                        };
                        let _ = tx.send(err_msg).await;
                        return Ok(());
                    }
                }
            } else {
                req.from_version
            };

            // Step 2: start watch forwarding task
            // Cancel any previously active watch task before starting a new one.
            if let Some(prev) = active_watch_cancel.take() {
                prev.cancel();
            }

            let watch_id = format!("center-{}", req.request_id);
            match css.watch(&req.kind, watch_id, "center".to_string(), watch_from) {
                Ok(mut event_rx) => {
                    let cancel = CancellationToken::new();
                    *active_watch_cancel = Some(cancel.clone());

                    let tx = tx.clone();
                    let request_id = req.request_id.clone();
                    let watch_kind = req.kind.clone();
                    let initial_server_id = css.server_id();
                    let reload_conf_mgr = conf_mgr.clone();
                    tokio::spawn(async move {
                        let mut reload_check = tokio::time::interval(Duration::from_secs(1));
                        loop {
                            tokio::select! {
                                _ = cancel.cancelled() => {
                                    break; // Explicitly cancelled by a new WatchRequest
                                }
                                event = event_rx.recv() => {
                                    match event {
                                        Some(event) => {
                                            let msg = ControllerMessage {
                                                payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                                                    request_id: request_id.clone(),
                                                    data: event.data,
                                                    sync_version: event.sync_version,
                                                    server_id: event.server_id,
                                                    error: event.err.unwrap_or_default(),
                                                })),
                                            };
                                            fed_metrics::record_watch_event(
                                                &watch_kind,
                                                fed_metrics::labels::direction::SENT,
                                            );
                                            if tx.send(msg).await.is_err() {
                                                break; // federation stream closed
                                            }
                                        }
                                        None => break, // channel closed
                                    }
                                }
                                _ = reload_check.tick() => {
                                    // Detect Controller reload: new CSS has a different server_id.
                                    // Without this, the forwarding task sits idle forever after reload
                                    // because the old EventStore stops receiving events.
                                    if let Some(new_css) = reload_conf_mgr.config_sync_server() {
                                        if new_css.server_id() != initial_server_id {
                                            tracing::info!(
                                                component = "fed_client",
                                                request_id = %request_id,
                                                old_server_id = %initial_server_id,
                                                new_server_id = %new_css.server_id(),
                                                "Controller reloaded, sending ServerReload to trigger Center re-watch"
                                            );
                                            let msg = ControllerMessage {
                                                payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                                                    request_id: request_id.clone(),
                                                    error: "ServerReload".to_string(),
                                                    ..Default::default()
                                                })),
                                            };
                                            let _ = tx.send(msg).await;
                                            break;
                                        }
                                    }
                                    // If config_sync_server() returns None, reload is in progress — keep waiting
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    let err_msg = ControllerMessage {
                        payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                            request_id: req.request_id,
                            error: e,
                            ..Default::default()
                        })),
                    };
                    let _ = tx.send(err_msg).await;
                }
            }
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
