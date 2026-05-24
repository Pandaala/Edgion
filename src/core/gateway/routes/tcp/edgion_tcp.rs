use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use pingora_core::upstreams::peer::BasicPeer;

use crate::core::gateway::backends::{
    resolve_target_port_key, select_roundrobin_backend_key, validate_endpoint_backend_addr,
};
use crate::core::gateway::observe::{log_tcp, TcpLogEntry};
use crate::core::gateway::plugins::stream::get_global_stream_plugin_store;
use crate::core::gateway::plugins::{StreamContext, StreamPluginResult};
use crate::core::gateway::routes::tcp::TcpPortRouteManager;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::ListenerKey;

/// TCP connection context
pub struct TcpContext {
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    pub upstream_addr: Option<String>,
    pub start_time: Instant,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: TcpStatus,
    pub connection_established: bool,
    /// Bounded reason for a fail-close / policy denial, surfaced into the TCP
    /// access log. Stays `&'static str` (no dynamic content) so it never
    /// carries client-controlled data onto the logging path.
    pub deny_reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub enum TcpStatus {
    Success,
    NoRouteMatched,
    NoRuleAvailable,
    PluginDenied,
    NoBackendSelected,
    NoBackendResolved,
    UnsafeUpstreamAddress,
    UpstreamConnectionFailed,
    UpstreamReadError,
    UpstreamWriteError,
    DownstreamReadError,
    DownstreamWriteError,
}

/// TCP proxy service for Gateway TCP listeners.
pub struct EdgionTcpProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub tcp_route_manager: Arc<TcpPortRouteManager>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTcpProxy {
    async fn process_new(self: &Arc<Self>, downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        if *shutdown.borrow() {
            return None;
        }

        let (client_addr, client_port) = downstream
            .get_socket_digest()
            .and_then(|d| d.peer_addr().cloned())
            .and_then(|addr| addr.as_inet().map(|inet| (inet.ip().to_string(), inet.port())))
            .unwrap_or_else(|| ("unknown".to_string(), 0));

        let mut ctx = TcpContext {
            listener_port: self.listener_port,
            client_addr,
            client_port,
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TcpStatus::Success,
            connection_established: false,
            deny_reason: None,
        };

        self.handle_connection(downstream, &mut ctx).await;

        self.log_connection(&ctx).await;

        None
    }
}

impl EdgionTcpProxy {
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TcpContext) {
        // 1. Match TCPRoute from per-port route table
        let route_table = self.tcp_route_manager.load_route_table();
        let tcp_route = match route_table.match_route() {
            Some(route) => route,
            None => {
                ctx.status = TcpStatus::NoRouteMatched;
                return;
            }
        };

        // 2. Get the first rule
        let rule = match tcp_route.spec.rules.as_ref().and_then(|rules| rules.first()) {
            Some(rule) => rule,
            None => {
                ctx.status = TcpStatus::NoRuleAvailable;
                return;
            }
        };

        // 3. Execute stream plugins (dynamic lookup from StreamPluginStore)
        if let StreamPluginCheck::Deny(reason) = check_stream_plugins(
            rule.stream_plugin_store_key.as_deref(),
            &ctx.client_addr,
            self.listener_port,
        )
        .await
        {
            ctx.status = TcpStatus::PluginDenied;
            ctx.deny_reason = Some(reason);
            return;
        }

        // 4. Select backend
        let backend_ref = match rule.backend_finder.select() {
            Ok(backend) => backend,
            Err(_) => {
                ctx.status = TcpStatus::NoBackendSelected;
                return;
            }
        };

        // 5. Resolve backend address via EndpointSlice
        let route_ns = tcp_route.metadata.namespace.as_deref().unwrap_or("default");
        let backend_ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
        let service_key = format!("{}/{}", backend_ns, backend_ref.name);

        let backend = match select_roundrobin_backend_key(&service_key) {
            Some(backend) => backend,
            None => {
                ctx.status = TcpStatus::NoBackendResolved;
                return;
            }
        };

        // 6. Build upstream address
        let mut upstream_addr = backend.inner().addr.clone();
        resolve_target_port_key(&mut upstream_addr, backend_ref.port, &service_key);
        let upstream_addr_str = upstream_addr.to_string();
        ctx.upstream_addr = Some(upstream_addr_str.clone());
        if validate_endpoint_backend_addr(&upstream_addr, self.allow_loopback_upstream()).is_err() {
            ctx.status = TcpStatus::UnsafeUpstreamAddress;
            return;
        }

        // 7. Connect to upstream
        let peer = BasicPeer::new(&upstream_addr_str);
        let upstream = match self.connector.new_stream(&peer).await {
            Ok(stream) => stream,
            Err(_) => {
                ctx.status = TcpStatus::UpstreamConnectionFailed;
                return;
            }
        };

        ctx.connection_established = true;

        // 8. Bidirectional data forwarding
        self.duplex(downstream, upstream, ctx).await;
    }

    async fn duplex(&self, mut downstream: Stream, mut upstream: Stream, ctx: &mut TcpContext) {
        const BUFFER_SIZE: usize = 8192;
        let mut upstream_buf = vec![0u8; BUFFER_SIZE];
        let mut downstream_buf = vec![0u8; BUFFER_SIZE];

        loop {
            select! {
                result = downstream.read(&mut upstream_buf) => {
                    match result {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            ctx.bytes_sent += n as u64;
                            if (upstream.write_all(&upstream_buf[0..n]).await).is_err() {
                                ctx.status = TcpStatus::UpstreamWriteError;
                                break;
                            }
                        }
                        Err(_) => {
                            ctx.status = TcpStatus::DownstreamReadError;
                            break;
                        }
                    }
                }
                result = upstream.read(&mut downstream_buf) => {
                    match result {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            ctx.bytes_received += n as u64;
                            if (downstream.write_all(&downstream_buf[0..n]).await).is_err() {
                                ctx.status = TcpStatus::DownstreamWriteError;
                                break;
                            }
                            // Listener streams buffer writes; flush client-facing bytes promptly.
                            if (downstream.flush().await).is_err() {
                                ctx.status = TcpStatus::DownstreamWriteError;
                                break;
                            }
                        }
                        Err(_) => {
                            ctx.status = TcpStatus::UpstreamReadError;
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn log_connection(&self, ctx: &TcpContext) {
        let log_entry = TcpLogEntry::from_context(ctx);
        log_tcp(&log_entry).await;
    }

    fn allow_loopback_upstream(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .map(|s| s.allow_loopback_upstream)
            .unwrap_or(false)
    }
}

/// Outcome of section-3 stream plugin enforcement on a TCP connection.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StreamPluginCheck {
    /// Connection may proceed to backend selection.
    Allow,
    /// Connection must be rejected. The caller sets `TcpStatus::PluginDenied`
    /// and records the bounded `&'static str` reason into the access log.
    Deny(&'static str),
}

/// Stage-1 stream plugin enforcement for TCPRoute.
///
/// Fail-close: if a rule references an `EdgionStreamPlugins` (ESP) resource
/// but the policy cannot be evaluated (client IP unparseable, ESP missing
/// from the global store), reject the connection rather than admitting it.
/// Silently admitting traffic past a configured allowlist would be a
/// security bypass. Startup-time sync race is gated by
/// `ConfigClient::is_ready()` before listeners bind, so reaching the
/// missing-ESP branch implies misconfiguration or hot-delete.
///
/// Pass-through cases (return `Allow`):
/// - rule has no `stream_plugin_store_key` (no ESP attached);
/// - ESP exists but its runtime is empty (operator intent: declare an ESP
///   with no rules to act as a no-op).
pub(crate) async fn check_stream_plugins(
    store_key: Option<&str>,
    client_addr: &str,
    listener_port: u16,
) -> StreamPluginCheck {
    let Some(store_key) = store_key else {
        return StreamPluginCheck::Allow;
    };

    // Fail-close: data plane must not emit tracing on the per-connection hot
    // path. The bounded reason flows into the TCP access log via the caller,
    // mirroring the TLS path's `ctx.err_log` (routes/tls/mod.rs).
    let Ok(client_ip) = client_addr.parse() else {
        return StreamPluginCheck::Deny("client address unparseable");
    };

    let store = get_global_stream_plugin_store();
    let Some(resource) = store.get(store_key) else {
        return StreamPluginCheck::Deny("stream plugin resource not found");
    };

    let runtime = &resource.spec.stream_plugin_runtime;
    if runtime.is_empty() {
        // Empty runtime is operator intent — pass through silently.
        return StreamPluginCheck::Allow;
    }

    let stream_ctx = StreamContext::new(client_ip, listener_port);
    match runtime.run(&stream_ctx).await {
        StreamPluginResult::Allow => StreamPluginCheck::Allow,
        StreamPluginResult::Deny(_) => StreamPluginCheck::Deny("stream plugin policy denied"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fail_close_when_store_key_missing_in_tcp() {
        // Rule references an ESP but the global store does not contain it.
        // Must reject (fail-close) with the bounded reason.
        let store = get_global_stream_plugin_store();
        let key = "default/__tcp_fail_close_missing__";
        assert!(store.get(key).is_none(), "test precondition: key must not be in store");

        let result = check_stream_plugins(Some(key), "127.0.0.1", 9000).await;
        assert_eq!(
            result,
            StreamPluginCheck::Deny("stream plugin resource not found"),
            "missing ESP must fail-close"
        );
    }

    #[tokio::test]
    async fn fail_close_when_tcp_client_addr_unparseable() {
        // Rule references an ESP but client_addr is not a valid IP literal
        // (e.g. peer addr extraction returned "unknown"). Must reject.
        let result = check_stream_plugins(Some("default/something"), "unknown", 9000).await;
        assert_eq!(
            result,
            StreamPluginCheck::Deny("client address unparseable"),
            "unparseable client addr must fail-close"
        );
    }

    #[tokio::test]
    async fn pass_through_when_store_key_none() {
        // No ESP attached at all — section 3 is a no-op. Must allow.
        let result = check_stream_plugins(None, "127.0.0.1", 9000).await;
        assert_eq!(
            result,
            StreamPluginCheck::Allow,
            "rule with no store_key must pass through"
        );
    }
}
