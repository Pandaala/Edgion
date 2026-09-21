//! Listener builder module for adding different types of listeners to Pingora Server
//!
//! This module provides a clean abstraction for adding HTTP/HTTPS, TCP, UDP, and other
//! protocol listeners to the gateway server. It separates listener construction logic
//! from the main gateway bootstrap process.

use crate::runtime::server::proxy_protocol_policy::{
    get_global_proxy_protocol_policy_runtime, GuardFirstConnectionFilter, PreparedProxyProtocolPolicy,
};
use crate::runtime::ServerHeaderOpts;
use anyhow::{Context, Result};
use pingora_core::apps::HttpServerOptions;
use pingora_core::connectors::TransportConnector;
use pingora_core::listeners::{ConnectionFilter, Listeners, TcpSocketOptions};
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_core::services::listening::Service;
use pingora_proxy::{http_proxy_service, ProxyServiceBuilder};
use std::sync::Arc;
use tokio::net::UdpSocket;

use crate::observe::AccessLogger;
use crate::plugins::stream::StreamPluginConnectionFilter;
use crate::routes::http::{EdgionHttpProxy, EdgionHttpRedirectProxy};
use crate::routes::tcp::{get_global_tcp_route_managers, EdgionTcpProxy};
use crate::routes::tls::{get_global_tls_route_managers, EdgionTlsPassthroughProxy, EdgionTlsTcpProxy};
use crate::routes::udp::{get_global_udp_route_managers, get_udp_listener_registry, EdgionUdpProxy};
#[cfg(feature = "boringssl")]
use crate::tls::runtime::gateway::tls_pingora::TlsCallback;
use edgion_resources::constants::annotations::edgion;
use edgion_resources::resources::gateway::{GatewayTlsMode, Listener};
use edgion_resources::resources::proxy_protocol::ProxyProtocolPolicySourceKind;
use edgion_resources::ListenerKey;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Context passed to listener builders containing gateway-level information and listener config
#[derive(Clone)]
pub struct ListenerContext {
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,
    pub gateway_key: String,
    pub listener: Listener,
    pub listener_key: ListenerKey,
    pub access_logger: Arc<AccessLogger>,
    /// Shared binding to the GatewayClass EGC, or the distinct built-in default.
    pub egc: Arc<crate::config::GatewayConfigHandle>,
    pub server_conf: Arc<ServerConf>,
    /// Whether to enable HTTP/2 support (from Gateway annotation)
    pub enable_http2: bool,
    /// Gateway annotations
    pub gateway_annotations: std::collections::HashMap<String, String>,
    /// Startup-prepared socket policy for this physical listener.
    pub proxy_protocol_policy: PreparedProxyProtocolPolicy,
    /// Bounded source metadata for startup diagnostics.
    pub proxy_protocol_source: Option<ProxyProtocolPolicySourceKind>,
}

/// Pre-flight TCP bind probe.
///
/// Pingora's `add_tcp` / `add_tls_with_settings` / `Listeners::tcp` only queue a
/// `ServerAddress`; the real `bind()` + `listen()` is deferred to
/// `ServiceTrait::start_service` and a failure there manifests as
/// `.expect("Failed to build listeners")` panic inside a tokio task, with no
/// gateway / listener name in the message. Probing the bind synchronously here
/// turns startup-time port conflicts (EADDRINUSE) and permission errors
/// (EACCES) into a `Result::Err` carrying gateway / listener / address
/// context, matching the UDP path. The probe socket is dropped immediately;
/// Pingora re-binds with SO_REUSEADDR when the service starts.
fn probe_tcp_bind(addr: &str, gateway_key: &str, listener_name: &str) -> Result<()> {
    std::net::TcpListener::bind(addr).with_context(|| {
        format!(
            "gateway '{}' listener '{}' failed to bind {}",
            gateway_key, listener_name, addr
        )
    })?;
    Ok(())
}

/// Socket options for this listener, or `None` when there is nothing to set.
///
/// Returning `None` rather than a defaulted struct keeps the bind path
/// byte-for-byte what it was before PROXY protocol support existed.
fn tcp_sock_opts(context: &ListenerContext) -> Option<TcpSocketOptions> {
    tcp_sock_opts_for_policy(&context.proxy_protocol_policy)
}

fn tcp_sock_opts_for_policy(policy: &PreparedProxyProtocolPolicy) -> Option<TcpSocketOptions> {
    match policy {
        PreparedProxyProtocolPolicy::Disabled => None,
        PreparedProxyProtocolPolicy::Required => {
            let mut opts = TcpSocketOptions::default();
            opts.proxy_protocol = true;
            // Explicit Required is the only state that maps to Pingora's
            // mandatory `None` trust mode.
            opts.proxy_protocol_trusted_sources = None;
            Some(opts)
        }
        PreparedProxyProtocolPolicy::TrustedSources { trust, .. } => {
            let mut opts = TcpSocketOptions::default();
            opts.proxy_protocol = true;
            opts.proxy_protocol_trusted_sources = Some(trust.clone());
            Some(opts)
        }
    }
}

/// Bind a plain TCP endpoint on a service, honouring the PROXY protocol flag.
fn add_tcp_endpoint<A>(service: &mut Service<A>, addr: &str, context: &ListenerContext) {
    match tcp_sock_opts(context) {
        Some(opts) => service.add_tcp_with_settings(addr, opts),
        None => service.add_tcp(addr),
    }
}

/// Build a single-endpoint [`Listeners`] for the services that construct their
/// own listener set instead of using [`Service::add_tcp`].
fn tcp_listeners(addr: &str, context: &ListenerContext) -> Listeners {
    let mut listeners = Listeners::new();
    match tcp_sock_opts(context) {
        Some(opts) => listeners.add_tcp_with_settings(addr, opts),
        None => listeners.add_tcp(addr),
    }
    listeners
}

/// Add an HTTP or HTTPS listener to the Pingora server
///
/// This function creates an HTTP proxy service and adds it to the server
/// with or without TLS based on the enable_tls parameter.
///
/// # Parameters
/// - `enable_tls`: Whether to enable TLS/HTTPS
/// - `enable_http2`: Whether to enable HTTP/2 support (h2c for HTTP, ALPN for HTTPS)
pub fn add_http_listener(
    server: &mut Server,
    context: &ListenerContext,
    enable_tls: bool,
    enable_http2: bool,
) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);

    // Check if HTTP to HTTPS redirect is enabled (only for non-TLS listeners)
    let http_to_https_redirect = !enable_tls
        && context
            .gateway_annotations
            .get(edgion::HTTP_TO_HTTPS_REDIRECT)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    if http_to_https_redirect {
        let https_port: u16 = context
            .gateway_annotations
            .get(edgion::HTTPS_REDIRECT_PORT)
            .and_then(|v| v.parse().ok())
            .unwrap_or(443);

        let redirect_handler =
            EdgionHttpRedirectProxy::new(https_port, context.access_logger.clone(), context.egc.clone());
        let mut http_service = http_proxy_service(&context.server_conf, redirect_handler);
        apply_connection_filter(&mut http_service, context);
        probe_tcp_bind(&addr, &context.gateway_key, &listener_name)?;
        add_tcp_endpoint(&mut http_service, &addr, context);

        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            https_port=%https_port,
            "Adding HTTP to HTTPS redirect listener"
        );

        server.add_service(http_service);
        return Ok(());
    }

    let http_route_manager =
        crate::routes::http::get_global_http_route_managers().get_or_create_listener_manager(&context.listener_key);
    let grpc_route_manager =
        crate::routes::grpc::get_global_grpc_route_managers().get_or_create_listener_manager(&context.listener_key);
    let edgion_http = EdgionHttpProxy {
        listener_port: context.listener.port as u16,
        is_https_listener: enable_tls,
        listener_key: context.listener_key.clone(),
        server_header_opts: {
            let mut opts = ServerHeaderOpts::default();
            if !enable_tls {
                opts.remove_hsts();
            }
            opts
        },
        access_logger: context.access_logger.clone(),
        egc: context.egc.clone(),
        enable_http2: context.enable_http2,
        preflight_handler: crate::routes::http::proxy_http::PreflightHandler,
        http_route_manager,
        grpc_route_manager,
    };

    // Build HttpServerOptions: h2c + downstream keepalive request limit
    let mut opts = HttpServerOptions::default();

    if !enable_tls && enable_http2 {
        opts.h2c = true;
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            "Enabled h2c (HTTP/2 Cleartext) support"
        );
    }

    // Each new connection captures its EGC keepalive budget in the HTTP hook.
    let mut http_service = ProxyServiceBuilder::new(&context.server_conf, edgion_http)
        .server_options(opts)
        .build();

    apply_connection_filter(&mut http_service, context);

    if enable_tls {
        #[cfg(feature = "boringssl")]
        {
            let port = context.listener.port as u16;
            let mut tls_settings = TlsCallback::new_tls_settings_with_callback(
                port,
                context.listener_key.clone(),
                context.egc.clone(),
                true,
            )?;
            // Enable HTTP/2 for HTTPS if enable_http2 is true
            if enable_http2 {
                tls_settings.enable_h2();
            }
            probe_tcp_bind(&addr, &context.gateway_key, &listener_name)?;
            http_service.add_tls_with_settings(&addr, tcp_sock_opts(context), tls_settings);
            let protocol = if enable_http2 {
                "HTTPS (HTTP/2 enabled)"
            } else {
                "HTTPS"
            };
            tracing::info!(
                gateway=%context.gateway_key,
                listener=%listener_name,
                addr=%addr,
                protocol=%protocol,
                "Adding TLS listener"
            );
        }

        #[cfg(not(feature = "boringssl"))]
        {
            anyhow::bail!("TLS support requires the 'boringssl' feature");
        }
    } else {
        probe_tcp_bind(&addr, &context.gateway_key, &listener_name)?;
        add_tcp_endpoint(&mut http_service, &addr, context);
        let protocol = if enable_http2 { "HTTP (h2c enabled)" } else { "HTTP" };
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol=%protocol,
            "Adding TCP listener"
        );
    }

    server.add_service(http_service);

    Ok(())
}

/// Add a TCP listener to the Pingora server
pub fn add_tcp_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tcp_route_manager = get_global_tcp_route_managers().get_or_create_listener_manager(&context.listener_key);

    let edgion_tcp = EdgionTcpProxy {
        listener_port: port,
        listener_key: context.listener_key.clone(),
        tcp_route_manager,
        egc: context.egc.clone(),
        connector: TransportConnector::new(None),
    };

    probe_tcp_bind(&addr, &context.gateway_key, &listener_name)?;
    let mut tcp_service = Service::with_listeners(
        format!("TCP-{}", listener_name),
        tcp_listeners(&addr, context),
        edgion_tcp,
    );

    apply_connection_filter(&mut tcp_service, context);

    server.add_service(tcp_service);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="TCP",
        "Adding TCP listener"
    );

    Ok(())
}

/// Add a UDP listener to the Pingora server
///
/// UDP listeners don't use Pingora's Service abstraction — they run as
/// independent tokio tasks.  Each listener gets a per-port route manager
/// (mirroring TLS pattern) and a `CancellationToken` for graceful shutdown.
pub fn add_udp_listener(_server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let udp_route_manager = get_global_udp_route_managers().get_or_create_listener_manager(&context.listener_key);

    let socket = std::net::UdpSocket::bind(&addr)?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;

    let cancel_token = CancellationToken::new();
    let child_tasks = TaskTracker::new();

    let edgion_udp = Arc::new(EdgionUdpProxy::new(
        port,
        context.listener_key.clone(),
        udp_route_manager,
        context.egc.clone(),
        socket,
        cancel_token.clone(),
        child_tasks.clone(),
    ));

    get_udp_listener_registry().spawn(&context.listener_key, cancel_token, child_tasks, async move {
        edgion_udp.serve().await
    })?;

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="UDP",
        "Adding UDP listener"
    );

    Ok(())
}

/// Add a TLS terminate to TCP listener to the Pingora server
///
/// This function creates a TLS listener that terminates TLS and forwards
/// plain TCP traffic to backend services based on SNI routing.
#[cfg(feature = "boringssl")]
pub fn add_tls_terminate_to_tcp_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tls_route_manager = get_global_tls_route_managers().get_or_create_listener_manager(&context.listener_key);

    let edgion_tls = EdgionTlsTcpProxy {
        listener_port: port,
        listener_key: context.listener_key.clone(),
        tls_route_manager,
        access_logger: context.access_logger.clone(),
        egc: context.egc.clone(),
        connector: TransportConnector::new(None),
    };

    // Create TLS settings with callback for certificate loading (with port for SNI lookup)
    let tls_settings =
        TlsCallback::new_tls_settings_with_callback(port, context.listener_key.clone(), context.egc.clone(), false)?;

    let mut tls_service = Service::new(format!("TLS-TCP-{}", listener_name), edgion_tls);

    apply_connection_filter(&mut tls_service, context);

    probe_tcp_bind(&addr, &context.gateway_key, &listener_name)?;
    // Only bind the TLS endpoint; Listeners::tcp() would bind a plain TCP socket
    // on the same port, causing EEXIST when add_tls_with_settings tries to bind again.
    tls_service.add_tls_with_settings(&addr, tcp_sock_opts(context), tls_settings);

    server.add_service(tls_service);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="TLS-TCP",
        "Adding TLS terminate to TCP listener"
    );

    Ok(())
}

/// Add a TLS Passthrough listener to the Pingora server.
///
/// This creates a **plain TCP** listener that extracts SNI from the TLS
/// ClientHello without performing a TLS handshake, then transparently
/// relays the encrypted traffic to the matched backend.
pub fn add_tls_passthrough_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tls_route_manager = get_global_tls_route_managers().get_or_create_listener_manager(&context.listener_key);

    let proxy = EdgionTlsPassthroughProxy {
        listener_port: port,
        listener_key: context.listener_key.clone(),
        tls_route_manager,
        access_logger: context.access_logger.clone(),
        egc: context.egc.clone(),
        connector: TransportConnector::new(None),
    };

    probe_tcp_bind(&addr, &context.gateway_key, &listener_name)?;
    let mut service = Service::with_listeners(
        format!("TLS-Passthrough-{}", listener_name),
        tcp_listeners(&addr, context),
        proxy,
    );

    apply_connection_filter(&mut service, context);

    server.add_service(service);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%listener_name,
        addr=%addr,
        protocol="TLS-Passthrough",
        "Adding TLS passthrough listener"
    );

    Ok(())
}

/// Main entry point for adding a listener to the server
///
/// This function dispatches to the appropriate listener builder based on the
/// protocol specified in the listener configuration.
pub fn add_listener(server: &mut Server, context: ListenerContext) -> Result<()> {
    match &context.proxy_protocol_policy {
        PreparedProxyProtocolPolicy::Disabled => {}
        PreparedProxyProtocolPolicy::Required => tracing::info!(
            gateway=%context.gateway_key,
            listener=%context.listener.name,
            port=%context.listener.port,
            policy_mode="required",
            policy_source=?context.proxy_protocol_source,
            "Inbound PROXY protocol enabled"
        ),
        PreparedProxyProtocolPolicy::TrustedSources { cidr_count, .. } => {
            tracing::info!(
                gateway=%context.gateway_key,
                listener=%context.listener.name,
                port=%context.listener.port,
                policy_mode="trustedSources",
                policy_source=?context.proxy_protocol_source,
                trusted_source_count=*cidr_count,
                "Inbound PROXY protocol enabled"
            );
        }
    }
    if !matches!(&context.proxy_protocol_policy, PreparedProxyProtocolPolicy::Disabled)
        && context.gateway_annotations.contains_key(edgion::EDGION_STREAM_PLUGINS)
    {
        tracing::warn!(
            gateway=%context.gateway_key,
            listener=%context.listener.name,
            "edgion-stream-plugins runs before the PROXY header is read, so its IP rules see the load balancer address, not the real client"
        );
    }

    match context.listener.protocol.to_uppercase().as_str() {
        "HTTP" => add_http_listener(server, &context, false, context.enable_http2),
        "HTTPS" => add_http_listener(server, &context, true, context.enable_http2),
        "TCP" => add_tcp_listener(server, &context),
        "UDP" => add_udp_listener(server, &context),
        "TLS" => {
            // Listener TLS mode: Passthrough or Terminate (default).
            // CRD/serde already reject any other value; the controller filters
            // unsupported combinations (e.g. mixed-mode listeners on the same
            // port) before pushing config to the gateway.
            let tls_mode = context.listener.tls.as_ref().and_then(|t| t.mode).unwrap_or_default();

            match tls_mode {
                GatewayTlsMode::Passthrough => add_tls_passthrough_listener(server, &context),
                GatewayTlsMode::Terminate => {
                    // Terminate mode — default to TCP backend (TLSRoute semantics).
                    // `protocol: TLS` + `mode: Terminate` inherently means TLS→TCP;
                    // HTTPS already covers TLS→HTTP. No annotation required.
                    #[cfg(feature = "boringssl")]
                    {
                        add_tls_terminate_to_tcp_listener(server, &context)
                    }

                    #[cfg(not(feature = "boringssl"))]
                    {
                        anyhow::bail!("TLS protocol requires the 'boringssl' feature")
                    }
                }
            }
        }
        protocol => {
            anyhow::bail!("Unsupported protocol: {}", protocol)
        }
    }
}

/// Apply the always-on policy guard followed by an optional StreamPlugins
/// filter. Pingora's locked listener implementation invokes this composite in
/// `ListenerEndpoint::accept()` before `UninitializedStream::handshake()` calls
/// `proxy_protocol::apply()`.
///
/// The annotation value must be an unqualified resource name and is expanded with the
/// Gateway's namespace. Route-level StreamPlugins references have a separate contract
/// and may still use a qualified `namespace/name` value.
///
/// The filter runs at TCP level, before TLS handshake or HTTP parsing.
fn apply_connection_filter<A>(service: &mut Service<A>, context: &ListenerContext) {
    let stream_filter = build_stream_connection_filter(context);
    let filter = Arc::new(GuardFirstConnectionFilter::new(
        get_global_proxy_protocol_policy_runtime(),
        stream_filter,
    ));
    service.set_connection_filter(filter);
}

fn build_stream_connection_filter(context: &ListenerContext) -> Option<Arc<dyn ConnectionFilter>> {
    Some(Arc::new(StreamPluginConnectionFilter::new(
        context.listener_key.clone(),
        context.listener.port as u16,
    )))
}

#[cfg(test)]
mod tests {
    use crate::routes::common::resolve_gateway_stream_plugin_store_key;

    #[test]
    fn gateway_stream_plugin_store_key_uses_gateway_namespace() {
        assert_eq!(
            resolve_gateway_stream_plugin_store_key("  policy  ", "app").unwrap(),
            Some("app/policy".to_string())
        );
    }

    #[test]
    fn gateway_stream_plugin_store_key_rejects_qualified_name() {
        assert!(resolve_gateway_stream_plugin_store_key("shared/policy", "app").is_err());
    }
}

#[cfg(test)]
mod proxy_protocol_trust_tests {
    use std::net::SocketAddr;

    use pingora_core::protocols::l4::proxy_protocol::ProxyProtocolTrust;

    use super::*;

    #[derive(Debug)]
    struct AlwaysTrust;

    impl ProxyProtocolTrust for AlwaysTrust {
        fn is_trusted(&self, _addr: &SocketAddr) -> bool {
            true
        }
    }

    #[test]
    fn socket_mapping_has_one_mandatory_none_case() {
        assert!(tcp_sock_opts_for_policy(&PreparedProxyProtocolPolicy::Disabled).is_none());

        let required = tcp_sock_opts_for_policy(&PreparedProxyProtocolPolicy::Required).unwrap();
        assert!(required.proxy_protocol);
        assert!(required.proxy_protocol_trusted_sources.is_none());

        let trusted = tcp_sock_opts_for_policy(&PreparedProxyProtocolPolicy::TrustedSources {
            trust: Arc::new(AlwaysTrust),
            cidr_count: 1,
        })
        .unwrap();
        assert!(trusted.proxy_protocol);
        assert!(trusted.proxy_protocol_trusted_sources.is_some());
    }
}
