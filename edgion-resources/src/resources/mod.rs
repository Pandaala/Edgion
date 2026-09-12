//! Kubernetes resource type definitions
//!
//! This module contains all custom resource definitions and Gateway API types

pub mod allowed_san;
pub mod backend_tls_policy;
pub mod circuit_breaker;
pub mod common;
pub mod config_data_ref;
pub mod edgion_acme;
pub mod edgion_backend;
pub mod edgion_backend_traffic_policy;
pub mod edgion_config_data;
pub mod edgion_gateway_config;
pub mod edgion_plugins;
pub mod edgion_stream_plugins;
pub mod edgion_tls;
pub mod extension_ref;
pub mod gateway;
pub mod gateway_class;
pub mod grpc_route;
pub mod header_modifier;
pub mod health_check;
pub mod http_route;
pub mod http_route_ai_admission;
pub mod http_route_preparse;
pub mod lb_policy;
pub mod link_sys;
pub mod outlier_detection;
pub mod plugin_policy;
pub mod proxy_protocol;
pub mod reference_grant;
pub mod tcp_route;
pub mod tls_route;
pub mod udp_route;
pub mod upstream_authority;

// Re-export all resource types
pub use self::allowed_san::*;
pub use self::backend_tls_policy::*;
pub use self::common::*;
pub use self::edgion_acme::*;
pub use self::edgion_backend::*;
pub use self::edgion_backend_traffic_policy::*;
pub use self::edgion_config_data::*;
pub use self::edgion_gateway_config::*;
pub use self::edgion_plugins::*;
pub use self::edgion_stream_plugins::*;
pub use self::edgion_tls::*;
pub use self::gateway::*;
pub use self::gateway_class::*;
pub use self::grpc_route::*;
pub use self::health_check::*;
pub use self::http_route::*;
pub use self::lb_policy::*;
pub use self::link_sys::*;
pub use self::proxy_protocol::*;
pub use self::reference_grant::*;
pub use self::tcp_route::*;
pub use self::tls_route::*;
pub use self::udp_route::*;
pub use plugin_policy::{canonical_type_names, parse_qualified_name, PluginFamily, PluginPolicyAction};
