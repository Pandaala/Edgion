//! HTTP Routes and Proxy Implementation
//!
//! This module contains all HTTP-related functionality:
//! - Route matching and management ([`routes_mgr`])
//! - HTTP proxy implementation ([`edgion_http`])
//! - Pingora ProxyHttp trait implementation ([`edgion_http_pingora`])
//! - Request/response processing
//! - Plugin execution
//! - Access logging

pub(crate) mod conf_handler_impl;
pub mod lb_policy_sync;
pub mod match_engine;
pub mod match_unit;
pub mod redirect_http;
pub mod routes_mgr;

#[cfg(test)]
mod tests;

// HTTP proxy module
pub mod proxy_http;

pub use conf_handler_impl::create_route_manager_handler;
pub use match_unit::HttpRouteRuleUnit;
pub use redirect_http::EdgionHttpRedirectProxy;
pub use routes_mgr::{get_global_route_manager, DomainRouteRules, HttpRouteManagerStats, RouteManager};

// Re-export HTTP proxy types
pub use proxy_http::EdgionHttpProxy;
