//! Listener builder module for adding different types of listeners to Pingora Server
//!
//! This module provides a clean abstraction for adding HTTP/HTTPS, TCP, UDP, and other
//! protocol listeners to the gateway server. It separates listener construction logic
//! from the main gateway bootstrap process.

use anyhow::Result;
use pingora_core::apps::HttpServerOptions;
use pingora_core::connectors::TransportConnector;
use pingora_core::listeners::Listeners;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_core::services::listening::Service;
use pingora_proxy::{http_proxy_service, ProxyServiceBuilder};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::UdpSocket;

use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::plugins::stream::{get_global_stream_plugin_store, StreamPluginConnectionFilter};
use crate::core::gateway::routes::http::{EdgionHttpProxy, EdgionHttpRedirectProxy};
use crate::core::gateway::routes::tcp::{get_global_tcp_route_manager, EdgionTcpProxy};
use crate::core::gateway::routes::tls::{get_global_tls_route_managers, EdgionTlsTcpProxy};
use crate::core::gateway::routes::udp::{get_global_udp_route_managers, EdgionUdpProxy};
#[cfg(any(feature = "boringssl", feature = "openssl"))]
use crate::core::gateway::tls::runtime::gateway::tls_pingora::TlsCallback;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::gateway::Listener;
use tokio_util::sync::CancellationToken;

/// Annotation key to control HTTP/2 support
/// Set to "false" to disable HTTP/2 (both h2c and ALPN)
/// Default: "true" (enabled)
pub const ANNOTATION_ENABLE_HTTP2: &str = "edgion.io/enable-http2";

/// Annotation key to specify backend protocol for TLS listeners
/// Set to "tcp" for TLS terminate to TCP backend
pub const ANNOTATION_BACKEND_PROTOCOL: &str = "edgion.io/backend-protocol";

/// Annotation key to enable HTTP to HTTPS redirect
/// Set to "true" to redirect all HTTP requests to HTTPS
/// Default: "false" (disabled)
pub const ANNOTATION_HTTP_TO_HTTPS_REDIRECT: &str = "edgion.io/http-to-https-redirect";

/// Annotation key to specify HTTPS redirect port
/// Default: 443
pub const ANNOTATION_HTTPS_REDIRECT_PORT: &str = "edgion.io/https-redirect-port";

/// Annotation key to reference an EdgionStreamPlugins resource for TCP-level connection filtering.
///
/// Value format: "namespace/name" pointing to an EdgionStreamPlugins resource.
/// When set, incoming TCP connections are filtered by the referenced stream plugins
/// **before** TLS handshake or HTTP parsing, providing efficient early rejection.
///
/// Example: `edgion.io/edgion-stream-plugins: "default/global-ip-filter"`
pub const ANNOTATION_EDGION_STREAM_PLUGINS: &str = "edgion.io/edgion-stream-plugins";

/// Context passed to listener builders containing gateway-level information and listener config
#[derive(Clone)]
pub struct ListenerContext {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,
    pub gateway_key: String,
    pub listener: Listener,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub server_conf: Arc<ServerConf>,
    /// Whether to enable HTTP/2 support (from Gateway annotation)
    pub enable_http2: bool,
    /// Gateway annotations
    pub gateway_annotations: std::collections::HashMap<String, String>,
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
            .get(ANNOTATION_HTTP_TO_HTTPS_REDIRECT)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    // If redirect is enabled, use the simple redirect handler
    if http_to_https_redirect {
        let https_port: u16 = context
            .gateway_annotations
            .get(ANNOTATION_HTTPS_REDIRECT_PORT)
            .and_then(|v| v.parse().ok())
            .unwrap_or(443);

        let redirect_handler = EdgionHttpRedirectProxy::new(https_port);
        let mut http_service = http_proxy_service(&context.server_conf, redirect_handler);
        apply_connection_filter(&mut http_service, context);
        http_service.add_tcp(&addr);

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

    // Pre-parse timeout configurations once at initialization
    let parsed_timeouts =
        crate::core::gateway::routes::http::proxy_http::ParsedTimeouts::from_config(&context.edgion_gateway_config);

    // Initialize RealIpExtractor from configuration
    let real_ip_extractor = if let Some(real_ip_config) = &context.edgion_gateway_config.spec.real_ip {
        match crate::core::common::utils::RealIpExtractor::new(
            &real_ip_config.trusted_ips,
            real_ip_config.real_ip_header.clone(),
        ) {
            Ok(extractor) => Some(Arc::new(extractor)),
            Err(e) => {
                tracing::warn!(
                    gateway = %context.gateway_name,
                    listener = %listener_name,
                    error = ?e,
                    "Failed to build RealIpExtractor, real IP extraction disabled"
                );
                None
            }
        }
    } else {
        None
    };

    // Create preflight handler from gateway config
    let preflight_handler = crate::core::gateway::routes::http::proxy_http::PreflightHandler::new(
        context.edgion_gateway_config.spec.preflight_policy.clone(),
    );

    let edgion_http = EdgionHttpProxy {
        gateway_class_name: context.gateway_class_name.clone(),
        listener: context.listener.clone(),
        server_start_time: SystemTime::now(),
        server_header_opts: Default::default(),
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        parsed_timeouts,
        enable_http2: context.enable_http2,
        real_ip_extractor,
        preflight_handler,
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

    // Downstream keepalive request limit (per-connection, HTTP/1.1 only)
    // 0 means unlimited; non-zero sets the limit
    let limit = context
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|s| s.downstream_keepalive_request_limit)
        .unwrap_or(1000);
    if limit > 0 {
        opts.keepalive_request_limit = Some(limit);
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            limit=%limit,
            "Downstream keepalive request limit configured"
        );
    }

    let mut http_service = ProxyServiceBuilder::new(&context.server_conf, edgion_http)
        .server_options(opts)
        .build();

    // Apply connection filter if configured via annotation
    apply_connection_filter(&mut http_service, context);

    // Add listener with or without TLS
    if enable_tls {
        #[cfg(any(feature = "boringssl", feature = "openssl"))]
        {
            let port = context.listener.port as u16;
            let mut tls_settings =
                TlsCallback::new_tls_settings_with_callback(port, context.edgion_gateway_config.clone(), true)?;
            // Enable HTTP/2 for HTTPS if enable_http2 is true
            if enable_http2 {
                tls_settings.enable_h2();
            }
            http_service.add_tls_with_settings(&addr, None, tls_settings);
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

        #[cfg(not(any(feature = "boringssl", feature = "openssl")))]
        {
            anyhow::bail!("TLS support requires either 'boringssl' or 'openssl' feature");
        }
    } else {
        http_service.add_tcp(&addr);
        let protocol = if enable_http2 { "HTTP (h2c enabled)" } else { "HTTP" };
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol=%protocol,
            "Adding TCP listener"
        );
    }

    // Add service to server
    server.add_service(http_service);

    Ok(())
}

/// Add a TCP listener to the Pingora server
pub fn add_tcp_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    // Pre-fetch TCP routes for this gateway (similar to HTTP approach)
    let tcp_route_manager = get_global_tcp_route_manager();
    let namespace_str = context.gateway_namespace.as_deref().unwrap_or("");
    let gateway_tcp_routes = tcp_route_manager.get_or_create_gateway_tcp_routes(namespace_str, &context.gateway_name);

    // Create TCP proxy service
    let edgion_tcp = EdgionTcpProxy {
        gateway_name: context.gateway_name.clone(),
        gateway_namespace: context.gateway_namespace.clone(),
        listener_name: listener_name.clone(), // Pass listener name for sectionName matching
        listener_port: port,
        gateway_tcp_routes, // Pass in pre-fetched routes
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };

    // Create TCP service
    let mut tcp_service = Service::with_listeners(format!("TCP-{}", listener_name), Listeners::tcp(&addr), edgion_tcp);

    // Apply connection filter if configured via annotation
    apply_connection_filter(&mut tcp_service, context);

    // Add to server
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

    let udp_route_manager = get_global_udp_route_managers().get_or_create_port_manager(port);

    let socket = std::net::UdpSocket::bind(&addr)?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;

    let cancel_token = CancellationToken::new();
    let edgion_udp = Arc::new(EdgionUdpProxy::new(
        port,
        udp_route_manager,
        context.edgion_gateway_config.clone(),
        socket,
        cancel_token,
    ));

    tokio::spawn(async move {
        edgion_udp.serve().await;
    });

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
#[cfg(any(feature = "boringssl", feature = "openssl"))]
pub fn add_tls_terminate_to_tcp_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let listener_name = context.listener.name.clone();
    // Hostname is for SNI matching, not for binding - always bind to 0.0.0.0
    let addr = format!("0.0.0.0:{}", context.listener.port);
    let port = context.listener.port as u16;

    let tls_route_manager = get_global_tls_route_managers().get_or_create_port_manager(port);

    let edgion_tls = EdgionTlsTcpProxy {
        listener_port: port,
        tls_route_manager,
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };

    // Create TLS settings with callback for certificate loading (with port for SNI lookup)
    let tls_settings = TlsCallback::new_tls_settings_with_callback(port, context.edgion_gateway_config.clone(), false)?;

    let mut tls_service = Service::new(format!("TLS-TCP-{}", listener_name), edgion_tls);

    // Apply connection filter if configured via annotation
    apply_connection_filter(&mut tls_service, context);

    // Only bind the TLS endpoint; Listeners::tcp() would bind a plain TCP socket
    // on the same port, causing EEXIST when add_tls_with_settings tries to bind again.
    tls_service.add_tls_with_settings(&addr, None, tls_settings);

    // Add to server
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

/// Main entry point for adding a listener to the server
///
/// This function dispatches to the appropriate listener builder based on the
/// protocol specified in the listener configuration.
pub fn add_listener(server: &mut Server, context: ListenerContext) -> Result<()> {
    match context.listener.protocol.to_uppercase().as_str() {
        "HTTP" => add_http_listener(server, &context, false, context.enable_http2),
        "HTTPS" => add_http_listener(server, &context, true, context.enable_http2),
        "TCP" => add_tcp_listener(server, &context),
        "UDP" => add_udp_listener(server, &context),
        "TLS" => {
            #[cfg(any(feature = "boringssl", feature = "openssl"))]
            {
                // Check Gateway annotation for backend protocol
                let backend_protocol = context
                    .gateway_annotations
                    .get(ANNOTATION_BACKEND_PROTOCOL)
                    .map(|s| s.as_str());

                match backend_protocol {
                    Some("tcp") => add_tls_terminate_to_tcp_listener(server, &context),
                    _ => anyhow::bail!(
                        "TLS protocol requires '{}' annotation set to 'tcp'",
                        ANNOTATION_BACKEND_PROTOCOL
                    ),
                }
            }

            #[cfg(not(any(feature = "boringssl", feature = "openssl")))]
            {
                anyhow::bail!("TLS protocol requires either 'boringssl' or 'openssl' feature")
            }
        }
        protocol => {
            anyhow::bail!("Unsupported protocol: {}", protocol)
        }
    }
}

/// Apply connection filter to a Pingora Service if the Gateway has the
/// `edgion.io/edgion-stream-plugins` annotation set.
///
/// The annotation value should be "namespace/name" referencing an EdgionStreamPlugins resource.
/// The filter runs at TCP level, before TLS handshake or HTTP parsing.
fn apply_connection_filter<A>(service: &mut Service<A>, context: &ListenerContext) {
    let Some(annotation_value) = context.gateway_annotations.get(ANNOTATION_EDGION_STREAM_PLUGINS) else {
        return;
    };

    let store_key = annotation_value.trim().to_string();
    if store_key.is_empty() {
        return;
    }

    let store = get_global_stream_plugin_store();
    let port = context.listener.port as u16;

    let filter = Arc::new(StreamPluginConnectionFilter::new(store, store_key.clone(), port));
    service.set_connection_filter(filter);

    tracing::info!(
        gateway=%context.gateway_key,
        listener=%context.listener.name,
        store_key=%store_key,
        "ConnectionFilter enabled via edgion-stream-plugins annotation"
    );
}
