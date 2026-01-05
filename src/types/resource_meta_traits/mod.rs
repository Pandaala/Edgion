//! ResourceMeta trait and implementations
//!
//! This module provides the ResourceMeta trait for Kubernetes resources,
//! combining version information, resource kind, and type metadata.

mod backend_tls_policy;
mod edgion_gateway_config;
mod edgion_plugins;
mod edgion_stream_plugins;
mod edgion_tls;
mod endpoint;
mod endpoint_slice;
mod gateway;
mod gateway_class;
mod grpc_route;
mod http_route;
mod link_sys;
mod plugin_metadata;
mod reference_grant;
mod secret;
mod service;
mod tcp_route;
mod tls_route;
mod traits;
mod udp_route;

pub use traits::ResourceMeta;
