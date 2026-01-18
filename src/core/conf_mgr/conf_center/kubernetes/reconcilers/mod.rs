//! Reconciler functions for each Kubernetes resource type

pub mod backend_tls_policy;
pub mod edgion_gateway_config;
pub mod edgion_plugins;
pub mod edgion_stream_plugins;
pub mod edgion_tls;
pub mod endpoint_slice;
pub mod endpoints;
pub mod gateway;
pub mod gateway_class;
pub mod grpc_route;
pub mod http_route;
pub mod link_sys;
pub mod plugin_metadata;
pub mod reference_grant;
pub mod secret;
pub mod service;
pub mod tcp_route;
pub mod tls_route;
pub mod udp_route;

// Re-export all reconcile functions
pub use backend_tls_policy::reconcile as reconcile_backend_tls_policy;
pub use edgion_gateway_config::reconcile as reconcile_edgion_gateway_config;
pub use edgion_plugins::reconcile as reconcile_edgion_plugins;
pub use edgion_stream_plugins::reconcile as reconcile_edgion_stream_plugins;
pub use edgion_tls::reconcile as reconcile_edgion_tls;
pub use endpoint_slice::reconcile as reconcile_endpoint_slice;
pub use endpoints::reconcile as reconcile_endpoints;
pub use gateway::reconcile as reconcile_gateway;
pub use gateway_class::reconcile as reconcile_gateway_class;
pub use grpc_route::reconcile as reconcile_grpc_route;
pub use http_route::reconcile as reconcile_http_route;
pub use link_sys::reconcile as reconcile_link_sys;
pub use plugin_metadata::reconcile as reconcile_plugin_metadata;
pub use reference_grant::reconcile as reconcile_reference_grant;
pub use secret::reconcile as reconcile_secret;
pub use service::reconcile as reconcile_service;
pub use tcp_route::reconcile as reconcile_tcp_route;
pub use tls_route::reconcile as reconcile_tls_route;
pub use udp_route::reconcile as reconcile_udp_route;
