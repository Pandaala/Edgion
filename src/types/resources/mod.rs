//! Kubernetes resource type definitions
//!
//! This module contains all custom resource definitions and Gateway API types

pub mod edgion_gateway_config;
pub mod edgion_plugins;
pub mod edgion_tls;
pub mod gateway;
pub mod gateway_class;
pub mod http_route_preparse;
pub mod http_route;
pub mod grpc_route;
pub mod tcp_route;
pub mod udp_route;
pub mod tls_route;
pub mod plugin_metadata;
pub mod link_sys;

// Re-export all resource types
pub use self::edgion_gateway_config::*;
pub use self::edgion_plugins::*;
pub use self::edgion_tls::*;
pub use self::gateway::*;
pub use self::gateway_class::*;
pub use self::http_route_preparse::*;
pub use self::http_route::*;
pub use self::grpc_route::*;
pub use self::tcp_route::*;
pub use self::udp_route::*;
pub use self::tls_route::*;
pub use self::plugin_metadata::*;
pub use self::link_sys::*;

