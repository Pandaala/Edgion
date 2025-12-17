//! Listener builder module for adding different types of listeners to Pingora Server
//!
//! This module provides a clean abstraction for adding HTTP/HTTPS, TCP, UDP, and other
//! protocol listeners to the gateway server. It separates listener construction logic
//! from the main gateway bootstrap process.

use anyhow::Result;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use pingora_proxy::http_proxy_service;
use std::sync::Arc;
use std::time::SystemTime;

use crate::core::gateway::edgion_http::EdgionHttp;
use crate::core::observe::AccessLogger;
use crate::core::routes::DomainRouteRules;
use crate::core::tls::tls_pingora::TlsCallback;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::gateway::Listener;

/// Context passed to listener builders containing gateway-level information
#[derive(Clone)]
pub struct ListenerContext {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,
    pub gateway_key: String,
    pub domain_routes: Arc<DomainRouteRules>,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

/// Parameters for creating an HTTP/HTTPS listener
pub struct HttpListenerParams {
    pub gateway_class_name: Option<String>,
    pub gateway_namespace: Option<String>,
    pub gateway_name: String,
    pub gateway_key: String,
    pub listener: Listener,
    pub domain_routes: Arc<DomainRouteRules>,
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

/// Add an HTTP or HTTPS listener to the Pingora server
///
/// This function creates an EdgionHttp proxy service and adds it to the server
/// with or without TLS based on the listener configuration.
pub fn add_http_listener(
    server: &mut Server,
    server_conf: &Arc<ServerConf>,
    params: HttpListenerParams,
) -> Result<()> {
    let listener_name = params.listener.name.clone();
    let host = params.listener.hostname.as_deref().unwrap_or("0.0.0.0");
    let addr = format!("{}:{}", host, params.listener.port);

    // Determine if TLS should be enabled
    let enable_tls = params.listener.tls.is_some() 
        || params.listener.port == 443 
        || params.listener.port == 8443;

    // Pre-parse timeout configurations once at initialization
    let parsed_timeouts = crate::core::gateway::edgion_http::ParsedTimeouts::from_config(
        &params.edgion_gateway_config
    );

    // Create EdgionHttp proxy handler
    let edgion_http = EdgionHttp {
        gateway_class_name: params.gateway_class_name,
        gateway_namespace: params.gateway_namespace,
        gateway_name: params.gateway_name,
        listener: params.listener,
        server_start_time: SystemTime::now(),
        server_header_opts: Default::default(),
        domain_routes: params.domain_routes,
        access_logger: params.access_logger,
        edgion_gateway_config: params.edgion_gateway_config,
        parsed_timeouts,
    };

    // Create HTTP proxy service
    let mut http_service = http_proxy_service(server_conf, edgion_http);

    // Add listener with or without TLS
    if enable_tls {
        let tls_settings = TlsCallback::new_tls_settings_with_callback(true)?;
        http_service.add_tls_with_settings(&addr, None, tls_settings);
        tracing::info!(
            gateway=%params.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol="HTTPS",
            "Adding TLS listener"
        );
    } else {
        http_service.add_tcp(&addr);
        tracing::info!(
            gateway=%params.gateway_key,
            listener=%listener_name,
            addr=%addr,
            protocol="HTTP",
            "Adding TCP listener"
        );
    }

    // Add service to server
    server.add_service(http_service);

    Ok(())
}

/// Add a TCP listener to the Pingora server
///
/// This is a placeholder for future TCP proxy implementation.
#[allow(dead_code)]
pub fn add_tcp_listener(
    _server: &mut Server,
    _server_conf: &Arc<ServerConf>,
    _context: &ListenerContext,
    _listener: &Listener,
) -> Result<()> {
    tracing::warn!(
        listener=%_listener.name,
        "TCP listener not yet supported, skipping"
    );
    Ok(())
}

/// Add a UDP listener to the Pingora server
///
/// This is a placeholder for future UDP proxy implementation.
#[allow(dead_code)]
pub fn add_udp_listener(
    _server: &mut Server,
    _server_conf: &Arc<ServerConf>,
    _context: &ListenerContext,
    _listener: &Listener,
) -> Result<()> {
    tracing::warn!(
        listener=%_listener.name,
        "UDP listener not yet supported, skipping"
    );
    Ok(())
}

/// Main entry point for adding a listener to the server
///
/// This function dispatches to the appropriate listener builder based on the
/// protocol specified in the listener configuration.
pub fn add_listener(
    server: &mut Server,
    server_conf: &Arc<ServerConf>,
    listener: &Listener,
    context: ListenerContext,
) -> Result<()> {
    match listener.protocol.to_uppercase().as_str() {
        "HTTP" | "HTTPS" => {
            let params = HttpListenerParams {
                gateway_class_name: context.gateway_class_name,
                gateway_namespace: context.gateway_namespace,
                gateway_name: context.gateway_name,
                gateway_key: context.gateway_key,
                listener: listener.clone(),
                domain_routes: context.domain_routes,
                access_logger: context.access_logger,
                edgion_gateway_config: context.edgion_gateway_config,
            };
            add_http_listener(server, server_conf, params)
        }
        "TCP" => {
            add_tcp_listener(server, server_conf, &context, listener)
        }
        "UDP" => {
            add_udp_listener(server, server_conf, &context, listener)
        }
        "GRPC" => {
            // GRPC is essentially HTTP/2, so treat it as HTTP for now
            tracing::info!(
                listener=%listener.name,
                "GRPC protocol detected, treating as HTTP/2"
            );
            let params = HttpListenerParams {
                gateway_class_name: context.gateway_class_name,
                gateway_namespace: context.gateway_namespace,
                gateway_name: context.gateway_name,
                gateway_key: context.gateway_key,
                listener: listener.clone(),
                domain_routes: context.domain_routes,
                access_logger: context.access_logger,
                edgion_gateway_config: context.edgion_gateway_config,
            };
            add_http_listener(server, server_conf, params)
        }
        protocol => {
            anyhow::bail!("Unsupported protocol: {}", protocol)
        }
    }
}

