//! ReferenceGrant Validation Wrapper
//!
//! Unified entry point for ReferenceGrant validation.
//! Re-exports validation functions from the ref_grant module with cleaner names.

use crate::types::resources::{GRPCRoute, HTTPRoute, TCPRoute, TLSRoute, UDPRoute};

/// Validate HTTPRoute with ReferenceGrant if validation is enabled
///
/// Returns a list of validation errors (empty if all references are allowed)
pub fn validate_http_route(route: &HTTPRoute) -> Vec<String> {
    crate::core::ref_grant::validate_http_route_if_enabled(route)
}

/// Validate GRPCRoute with ReferenceGrant if validation is enabled
///
/// Returns a list of validation errors (empty if all references are allowed)
pub fn validate_grpc_route(route: &GRPCRoute) -> Vec<String> {
    crate::core::ref_grant::validate_grpc_route_if_enabled(route)
}

/// Validate TCPRoute with ReferenceGrant if validation is enabled
///
/// Returns a list of validation errors (empty if all references are allowed)
pub fn validate_tcp_route(route: &TCPRoute) -> Vec<String> {
    crate::core::ref_grant::validate_tcp_route_if_enabled(route)
}

/// Validate UDPRoute with ReferenceGrant if validation is enabled
///
/// Returns a list of validation errors (empty if all references are allowed)
pub fn validate_udp_route(route: &UDPRoute) -> Vec<String> {
    crate::core::ref_grant::validate_udp_route_if_enabled(route)
}

/// Validate TLSRoute with ReferenceGrant if validation is enabled
///
/// Returns a list of validation errors (empty if all references are allowed)
pub fn validate_tls_route(route: &TLSRoute) -> Vec<String> {
    crate::core::ref_grant::validate_tls_route_if_enabled(route)
}
