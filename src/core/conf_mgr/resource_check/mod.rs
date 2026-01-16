//! Resource Check Module
//!
//! Unified entry point for resource validation and status generation.
//!
//! This module provides:
//! - `ResourceCheckContext`: Context for checking resource dependencies
//! - `check_edgion_tls`: EdgionTls validation (Gateway existence, etc.)
//! - `validate_*_route`: ReferenceGrant validation for routes
//! - `generate_*_status`: Status generation for K8s resources (event-driven)

mod context;
mod edgion_tls;
mod ref_grant;
mod status;

pub use context::ResourceCheckContext;
pub use edgion_tls::{check_edgion_tls, EdgionTlsCheckResult};
pub use ref_grant::{
    validate_grpc_route, validate_http_route, validate_tcp_route, validate_tls_route, validate_udp_route,
};
pub use status::{
    gateway_status_needs_update, generate_gateway_status, generate_http_route_status, http_route_status_needs_update,
    status_conditions_equal,
};
