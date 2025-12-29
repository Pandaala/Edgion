//! HTTP Routes and Proxy Implementation
//!
//! This module contains all HTTP-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - HTTP proxy implementation ([`edgion_http`])
//! - Pingora ProxyHttp trait implementation ([`edgion_http_pingora`])
//! - Request/response processing
//! - Plugin execution
//! - Access logging

pub mod match_engine;
pub mod match_unit;
pub mod routes_mgr;
pub mod lb_policy_sync;
mod conf_handler_impl;

#[cfg(test)]
mod tests;

// HTTP proxy module
pub mod proxy_http;

pub use match_unit::HttpRouteRuleUnit;
pub use routes_mgr::{RouteManager, DomainRouteRules, get_global_route_manager};
pub use conf_handler_impl::create_route_manager_handler;

// Re-export HTTP proxy types
pub use proxy_http::EdgionHttp;

