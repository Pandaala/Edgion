use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

use pingora_core::apps::ServerApp;
use pingora_core::connectors::TransportConnector;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
#[cfg(feature = "boringssl")]
use pingora_core::tls::ssl::NameType;
use pingora_core::upstreams::peer::BasicPeer;

use crate::backends::{
    select_service_endpoint, service_section_key_from_port, validate_endpoint_backend_addr, ServiceDegradePolicy,
};
use crate::observe::log_tls;
use crate::observe::logs::LogBuffer;
use crate::observe::AccessLogger;
use crate::routes::common::UNSAFE_UPSTREAM_ADDRESS_LOG;
use crate::routes::l4_duplex::bidirectional_relay;
use crate::routes::tls::{TlsRouteManager, TlsRouteUnit};
use crate::util::proxy_protocol::ProxyProtocolV2Builder;
use crate::{MatchedInfo, Rsc, TlsConnMeta};
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfig;
use edgion_resources::{ListenerKey, ResourceMeta};
use serde::Serialize;

use super::{tls_connect_retry_backoff, TlsDuplexHooks};

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct TlsUpstreamInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes_sent: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes_received: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_time: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub connection_established: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub proxy_protocol_sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_protocol: Option<String>,
}

/// TLS connection context
#[derive(Debug, Clone, Serialize)]
pub struct TlsContext {
    pub ts: i64,
    #[serde(skip_serializing)]
    pub start_at: Instant,
    pub listener_port: u16,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub direct_peer_ip: String,
    pub direct_peer_port: u16,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub remote_ip: String,
    pub remote_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<MatchedInfo>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_mtls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub upstream_info: Vec<TlsUpstreamInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err_log: Option<String>,
    /// Structured failure token (`RefDenied`, `UnsupportedBackend`, …).
    /// Complements the free-form `err_log` so operator pipelines can match
    /// TLS the same way they match TCP `status`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
}

impl TlsContext {
    pub(super) fn new(
        listener_port: u16,
        direct_peer_ip: String,
        direct_peer_port: u16,
        remote_ip: String,
        remote_port: u16,
    ) -> Self {
        Self {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            start_at: Instant::now(),
            listener_port,
            direct_peer_ip,
            direct_peer_port,
            remote_ip,
            remote_port,
            tls_id: None,
            sni: None,
            matched: None,
            is_mtls: false,
            end_time: None,
            upstream_info: Vec::new(),
            log: None,
            err_log: None,
            status: None,
        }
    }

    pub(super) fn push_log(&mut self, message: &str) {
        let _ = self.log.get_or_insert_with(LogBuffer::new).push(message);
    }

    pub(super) fn current_upstream_mut(&mut self) -> Option<&mut TlsUpstreamInfo> {
        self.upstream_info.last_mut()
    }
}

/// TLS-to-TCP proxy service for Gateway TLS listeners.
///
/// Each instance holds an `Arc<TlsRouteManager>` for its listener,
/// obtained at startup from `GlobalTlsRouteManagers`. Route updates swap
/// only the inner `ArcSwap<TlsRouteTable>`, so the Arc stays stable.
pub struct EdgionTlsTcpProxy {
    pub listener_port: u16,
    pub listener_key: ListenerKey,
    pub tls_route_manager: Arc<TlsRouteManager>,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}

#[async_trait]
impl ServerApp for EdgionTlsTcpProxy {
    async fn process_new(self: &Arc<Self>, mut downstream: Stream, shutdown: &ShutdownWatch) -> Option<Stream> {
        let digest = downstream.get_socket_digest();
        let addrs = crate::util::resolve_transport_client_addrs(
            digest.as_deref().and_then(|d| d.peer_addr()),
            digest.as_deref().and_then(|d| d.raw_peer_addr()),
            "unknown",
        );

        let mut ctx = TlsContext::new(
            self.listener_port,
            addrs.direct_peer_ip,
            addrs.direct_peer_port,
            addrs.remote_ip,
            addrs.remote_port,
        );

        if *shutdown.borrow() {
            ctx.err_log = Some("shutdown".to_string());
            self.log_disconnect(&mut ctx).await;
            return None;
        }

        // Borrow handshake metadata only while installing the connection fields.
        {
            let ssl_digest = downstream.get_ssl_digest();
            install_tls_metadata(
                &mut ctx,
                ssl_digest.as_ref().and_then(|d| d.extension.get::<TlsConnMeta>()),
            );
        }
        if ctx.sni.is_none() {
            ctx.sni = Self::extract_sni(&mut downstream);
        }

        let sni = match ctx.sni.as_deref() {
            Some(sni) => sni.to_string(),
            None => {
                ctx.err_log = Some("no SNI".to_string());
                self.log_disconnect(&mut ctx).await;
                return None;
            }
        };

        self.handle_connection(downstream, &mut ctx, &sni).await;
        self.log_disconnect(&mut ctx).await;

        None
    }
}

impl EdgionTlsTcpProxy {
    /// Extract SNI hostname from TLS stream
    fn extract_sni(#[allow(unused_variables)] stream: &mut Stream) -> Option<String> {
        #[cfg(feature = "boringssl")]
        {
            if let Some(ssl_ref) = stream.get_ssl() {
                if let Some(sni) = ssl_ref.servername(NameType::HOST_NAME) {
                    return Some(sni.to_string());
                }
            }
        }
        None
    }

    /// Core logic for handling TLS-terminated connections
    async fn handle_connection(&self, downstream: Stream, ctx: &mut TlsContext, sni: &str) {
        // 1. Match TLSRoute unit by SNI from the per-listener route table.
        let unit = match select_route_unit(&self.tls_route_manager, sni) {
            Some(unit) => unit,
            None => {
                ctx.err_log = Some("no match".to_string());
                return;
            }
        };
        let tls_route = &unit.route;

        ctx.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "TLSRoute".to_string(),
                ns: tls_route.metadata.namespace.clone().unwrap_or_default(),
                name: tls_route.metadata.name.clone().unwrap_or_default(),
                labels: unit.log_labels.clone(),
            },
            section: None,
            sv: tls_route.get_sync_version(),
        });

        // 2. Require at least one rule. The compiled runtime (backend_finder,
        // retries, keepalive, proxy-protocol, stream-plugin key) lives on the
        // unit and is built from the first rule.
        if tls_route.spec.rules.as_ref().and_then(|rules| rules.first()).is_none() {
            ctx.err_log = Some("no rules".to_string());
            return;
        }

        // 3. Execute TLS route plugins (Stage 2: post-handshake, post-route-match)
        if !self
            .run_tls_route_plugins(unit.stream_plugin_store_key.as_deref(), ctx, sni)
            .await
        {
            return;
        }

        // 4. Connect upstream with retry
        let max_attempts = unit.max_connect_retries;
        let mut upstream = None;
        let mut last_err = String::new();

        for attempt in 0..max_attempts {
            // 4a. Select backend (re-select on each retry for load-balancing rotation)
            let backend_ref = match unit.backend_finder.select() {
                Ok(b) => b,
                Err(_) => {
                    ctx.err_log = Some("no backend".to_string());
                    return;
                }
            };
            if super::apply_tls_l4_post_select(
                ctx,
                backend_ref.ref_denied.as_ref(),
                backend_ref.group.as_deref(),
                backend_ref.kind.as_deref(),
            ) {
                // Authorization or kind miss is terminal: do not retry onto
                // another backend and do not look up or dial the Service.
                return;
            }

            // 4b. Resolve backend address via EndpointSlice
            let route_ns = tls_route.metadata.namespace.as_deref().unwrap_or("default");
            let backend_ns = backend_ref.namespace.as_deref().unwrap_or(route_ns);
            let service_key = format!("{}/{}", backend_ns, backend_ref.name);

            let section = match service_section_key_from_port(backend_ref.port) {
                Ok(section) => section,
                Err(_) => {
                    last_err = format!("no proven section: {}", service_key);
                    continue;
                }
            };
            let endpoint =
                match select_service_endpoint(&service_key, section, &None, ServiceDegradePolicy::Live, None, "", None)
                {
                    Ok(endpoint) => endpoint,
                    Err(_) => {
                        last_err = format!("no endpoint: {}", service_key);
                        continue;
                    }
                };
            let backend = endpoint.backend;

            let upstream_addr = &backend.inner().addr;
            let upstream_addr_str = upstream_addr.to_string();
            let outlier_store = crate::backends::get_outlier_detection_store();
            let outlier_token = outlier_store.capture_completion_token(backend.as_ref());
            let parsed_upstream_addr: Option<std::net::SocketAddr> = upstream_addr_str.parse().ok();

            ctx.upstream_info.push(TlsUpstreamInfo {
                addr: parsed_upstream_addr.as_ref().map(|addr| addr.ip().to_string()),
                port: parsed_upstream_addr.as_ref().map(|addr| addr.port()),
                backend: Some(MatchedInfo {
                    rsc: Rsc {
                        kind: backend_ref.kind.clone().unwrap_or_else(|| "Service".to_string()),
                        ns: backend_ns.to_string(),
                        name: backend_ref.name.clone(),
                        labels: None,
                    },
                    section: None,
                    sv: 0,
                }),
                bytes_sent: 0,
                bytes_received: 0,
                connect_time: None,
                connection_established: false,
                proxy_protocol_sent: false,
                upstream_protocol: Some("TCP".to_string()),
            });
            if validate_endpoint_backend_addr(upstream_addr, self.allow_loopback_upstream()).is_err() {
                ctx.err_log = Some(UNSAFE_UPSTREAM_ADDRESS_LOG.to_string());
                return;
            }

            // 4c. TCP connect
            let mut peer = BasicPeer::new(&upstream_addr_str);
            peer.options.tcp_keepalive = unit
                .tcp_keepalive
                .as_ref()
                .map(crate::routes::common::to_pingora_tcp_keepalive);
            // Bound the upstream TCP connect so an unreachable backend fails
            // fast instead of stranding the downstream connection until the OS
            // TCP retransmit timeout; mirrors the HTTP path. The upstream connect
            // is plain TCP here (downstream TLS is terminated separately), so this
            // bounds the connect itself, not a TLS handshake.
            peer.options.connection_timeout =
                edgion_resources::resources::edgion_gateway_config::resolve_tcp_connect_timeout(
                    self.edgion_gateway_config.spec.tcp_timeout.as_ref(),
                );
            match self.connector.new_stream(&peer).await {
                Ok(stream) => {
                    if let Some(info) = ctx.current_upstream_mut() {
                        info.connect_time = Some(
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as i64,
                        );
                        info.connection_established = true;
                    }
                    // A successful connect is the L4 "backend reachable" success
                    // signal; it resets the endpoint's outlier-detection counters.
                    if let Some(token) = outlier_token.as_ref() {
                        outlier_store.record_success_for_backend_with_token(backend.as_ref(), token);
                    }
                    upstream = Some((stream, upstream_addr_str));
                    break;
                }
                Err(e) => {
                    // TLS upstream connect failure — local-origin failure.
                    if let Some(token) = outlier_token.as_ref() {
                        outlier_store.record_failure_for_backend_with_token(
                            backend.as_ref(),
                            token,
                            crate::backends::FailureClass::LocalOrigin,
                        );
                    }
                    last_err = format!("connect failed: {upstream_addr_str}: {e}");
                    if attempt + 1 < max_attempts {
                        ctx.push_log(&format!("retry {}/{max_attempts}: {last_err}", attempt + 1));
                        tokio::time::sleep(tls_connect_retry_backoff(attempt)).await;
                    }
                }
            }
        }

        let (mut upstream_stream, upstream_addr_str) = match upstream {
            Some(pair) => pair,
            None => {
                ctx.err_log = Some(last_err);
                return;
            }
        };

        // 5. Send Proxy Protocol v2 header if configured
        if !self
            .send_proxy_protocol_v2(
                unit.proxy_protocol_version,
                ctx,
                sni,
                &upstream_addr_str,
                &mut upstream_stream,
            )
            .await
        {
            return;
        }

        // 6. Bidirectional data forwarding
        self.duplex(downstream, upstream_stream, ctx).await
    }

    /// Send Proxy Protocol v2 header to upstream if PP2 is enabled.
    /// Returns `false` if the write fails (caller should abort the connection).
    async fn send_proxy_protocol_v2(
        &self,
        proxy_protocol_version: Option<u8>,
        ctx: &mut TlsContext,
        sni: &str,
        upstream_addr_str: &str,
        upstream: &mut Stream,
    ) -> bool {
        let Some(2) = proxy_protocol_version else {
            return true;
        };
        let Ok(src_ip) = ctx.remote_ip.parse::<IpAddr>() else {
            return true;
        };

        let src_addr = std::net::SocketAddr::new(src_ip, ctx.remote_port);
        let dst_addr: std::net::SocketAddr = upstream_addr_str
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0));
        let mut builder = ProxyProtocolV2Builder::new(src_addr, dst_addr);
        builder.add_authority(sni);
        let pp2_header = builder.build();

        if let Err(e) = upstream.write_all(&pp2_header).await {
            ctx.err_log = Some(format!("PP2 write failed: {e}"));
            return false;
        }
        if upstream.flush().await.is_err() {
            ctx.err_log = Some("PP2 flush failed".to_string());
            return false;
        }
        if let Some(info) = ctx.current_upstream_mut() {
            info.proxy_protocol_sent = true;
        }
        ctx.push_log("PP2 sent");
        true
    }

    /// Execute TLS route plugins (Stage 2).
    async fn run_tls_route_plugins(&self, store_key: Option<&str>, ctx: &mut TlsContext, sni: &str) -> bool {
        super::run_tls_route_plugins(store_key, ctx, sni, self.listener_port, super::TlsRouteMode::Terminated).await
    }

    /// Bidirectional data transfer between downstream (TLS-terminated) and upstream (TCP)
    async fn duplex(&self, downstream: Stream, upstream: Stream, ctx: &mut TlsContext) {
        let idle = edgion_resources::resources::edgion_gateway_config::resolve_tcp_idle_timeout(
            self.edgion_gateway_config.spec.tcp_timeout.as_ref(),
        );
        bidirectional_relay(downstream, upstream, TlsDuplexHooks { ctx }, idle).await;
    }

    fn is_tls_proxy_log_enabled(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .is_none_or(|sp| sp.tls_proxy_log_record)
    }

    fn allow_loopback_upstream(&self) -> bool {
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .map(|s| s.allow_loopback_upstream)
            .unwrap_or(false)
    }

    /// Log connection disconnection event
    async fn log_disconnect(&self, ctx: &mut TlsContext) {
        if !self.is_tls_proxy_log_enabled() {
            return;
        }
        ctx.end_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        );
        log_tls(ctx).await;

        // Also emit to the per-listener access logger
        let direct_peer_ip = edgion_resources::sensitive::apply_sensitive(&ctx.direct_peer_ip, str::to_owned);
        let remote_ip = edgion_resources::sensitive::apply_sensitive(&ctx.remote_ip, str::to_owned);
        let log_entry = serde_json::json!({
            "ts": ctx.ts,
            "listener_port": ctx.listener_port,
            "direct_peer_ip": direct_peer_ip,
            "direct_peer_port": ctx.direct_peer_port,
            "remote_ip": remote_ip,
            "remote_port": ctx.remote_port,
            "tls_id": &ctx.tls_id,
            "sni": &ctx.sni,
            "end_time": ctx.end_time,
            "upstream_info": &ctx.upstream_info,
            "log": &ctx.log,
            "err_log": &ctx.err_log,
            "status": ctx.status,
        });
        self.access_logger.send(log_entry.to_string()).await;
    }
}

/// Keep only the selected generation, never a table guard across connection I/O.
fn select_route_unit(manager: &TlsRouteManager, sni: &str) -> Option<Arc<TlsRouteUnit>> {
    manager.load_route_table().match_route(sni)
}

/// Copy only the fields used by the TLS connection, not the full handshake record.
fn install_tls_metadata(ctx: &mut TlsContext, meta: Option<&TlsConnMeta>) {
    if let Some(meta) = meta {
        ctx.tls_id = meta.tls_id.clone();
        ctx.sni = meta.sni.clone();
        ctx.matched = meta.matched.clone();
        ctx.is_mtls = meta.is_mtls;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::ClientCertInfo;
    use edgion_resources::resources::tls_route::{TLSRoute, TLSRouteSpec};
    use kube::api::ObjectMeta;
    use std::collections::HashMap;

    fn route_unit(name: &str, hostname: &str) -> Arc<TlsRouteUnit> {
        Arc::new(TlsRouteUnit::from_route(Arc::new(TLSRoute {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: None,
                hostnames: Some(vec![hostname.to_string()]),
                rules: None,
                resolved_listeners: None,
                effective_hostnames: None,
            },
            status: None,
        })))
    }

    #[test]
    fn selected_unit_survives_reload_without_retaining_old_table() {
        let manager = TlsRouteManager::new();
        assert!(select_route_unit(&manager, "a.example.com").is_none());
        let old_unit = route_unit("a", "a.example.com");
        let sibling = route_unit("b", "b.example.com");
        let old_unit_weak = Arc::downgrade(&old_unit);
        let selector_weak = Arc::downgrade(&old_unit.backend_finder);
        let sibling_weak = Arc::downgrade(&sibling);
        manager.rebuild(&HashMap::from([
            ("default/a".to_string(), old_unit),
            ("default/b".to_string(), sibling),
        ]));
        let old_table = Arc::downgrade(&manager.load_route_table());
        assert!(select_route_unit(&manager, "missing.example.com").is_none());
        let selected = select_route_unit(&manager, "A.EXAMPLE.COM").unwrap();
        assert!(Arc::ptr_eq(&selected, &old_unit_weak.upgrade().unwrap()));
        assert!(select_route_unit(&manager, "b.example.com").is_some());

        let new_unit = route_unit("a", "a.example.com");
        manager.rebuild(&HashMap::from([("default/a".to_string(), new_unit.clone())]));
        assert!(old_table.upgrade().is_none(), "selection must release the table guard");
        assert!(
            sibling_weak.upgrade().is_none(),
            "unselected hostname winner is released"
        );
        assert!(Arc::ptr_eq(&selected.backend_finder, &selector_weak.upgrade().unwrap()));
        assert!(Arc::ptr_eq(
            &select_route_unit(&manager, "a.example.com").unwrap(),
            &new_unit
        ));
        assert!(!Arc::ptr_eq(&selected, &new_unit));
        assert!(select_route_unit(&manager, "b.example.com").is_none());
        drop(selected);
        assert!(old_unit_weak.upgrade().is_none());
        assert!(selector_weak.upgrade().is_none());
    }

    #[test]
    fn metadata_install_preserves_fields_and_leaves_source_unchanged() {
        let mut ctx = TlsContext::new(8443, "192.0.2.1".into(), 1234, "192.0.2.2".into(), 5678);
        let mut meta = TlsConnMeta {
            ts: 42,
            start_at: Instant::now(),
            handshake_complete_time: Some(17),
            port: Some(9443),
            tls_id: Some("tls-connection".into()),
            sni: Some("A.EXAMPLE.COM".into()),
            log: None,
            matched: Some(MatchedInfo {
                rsc: Rsc {
                    kind: "EdgionTls".into(),
                    ns: "default".into(),
                    name: "certificate".into(),
                    labels: None,
                },
                section: Some("secure".into()),
                sv: 123,
            }),
            err_log: Some("handshake diagnostic".into()),
            client_cert_info: Some(ClientCertInfo {
                subject: "CN=client.example.com".into(),
                sans: vec!["client.example.com".into()],
                cn: Some("client.example.com".into()),
                fingerprint: "test-fingerprint".into(),
            }),
            is_mtls: true,
        };
        let before = serde_json::to_value(&meta).unwrap();
        install_tls_metadata(&mut ctx, Some(&meta));
        assert_eq!(ctx.tls_id, meta.tls_id);
        assert_eq!(ctx.sni, meta.sni);
        assert_eq!(serde_json::to_value(&ctx.matched).unwrap(), before["matched"]);
        assert!(ctx.is_mtls);
        assert_eq!(ctx.listener_port, 8443);
        assert!(ctx.log.is_none());
        assert!(ctx.err_log.is_none());
        assert_eq!(serde_json::to_value(&meta).unwrap(), before);
        let cert = meta.client_cert_info.as_ref().unwrap();
        assert_eq!(cert.subject, "CN=client.example.com");
        assert_eq!(cert.sans, ["client.example.com"]);
        assert_eq!(cert.cn.as_deref(), Some("client.example.com"));
        assert_eq!(cert.fingerprint, "test-fingerprint");

        let installed = serde_json::to_value(&ctx).unwrap();
        install_tls_metadata(&mut ctx, None);
        assert_eq!(serde_json::to_value(&ctx).unwrap(), installed);

        meta.tls_id = None;
        meta.sni = None;
        meta.matched = None;
        meta.is_mtls = false;
        install_tls_metadata(&mut ctx, Some(&meta));
        assert!(ctx.tls_id.is_none());
        assert!(ctx.sni.is_none());
        assert!(ctx.matched.is_none());
        assert!(!ctx.is_mtls);
    }
}
