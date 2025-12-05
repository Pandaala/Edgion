//! ResourceMeta trait and implementations
//!
//! This module provides the ResourceMeta trait for Kubernetes resources,
//! combining version information, resource kind, and type metadata.

mod traits;
mod gateway_class;
mod edgion_gateway_config;
mod gateway;
mod http_route;
mod service;
mod endpoint_slice;
mod secret;
mod edgion_tls;

pub use traits::ResourceMeta;

