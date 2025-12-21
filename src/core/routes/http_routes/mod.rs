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

// HTTP 代理模块
pub mod edgion_http;
pub mod edgion_http_pingora;

pub use match_unit::HttpRouteRuleUnit;
pub use routes_mgr::{RouteManager, DomainRouteRules, get_global_route_manager};
pub use conf_handler_impl::create_route_manager_handler;

// 导出 HTTP 代理类型
pub use edgion_http::EdgionHttp;

