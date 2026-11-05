//! HTTP Routes and Proxy Implementation
//!
//! - Route matching and management ([`routes_mgr`])
//! - HTTP proxy ([`proxy_http`])
//! - HTTP-to-HTTPS redirect ([`redirect_http`])
//! - Session persistence ([`session_persistence`])

pub(crate) mod conf_handler_impl;
pub mod delegation_build;
pub(crate) mod match_engine;
pub mod match_unit;
pub mod redirect_http;
pub mod routes_mgr;

#[cfg(test)]
#[path = "../../../tests/unit/routes/http/mod_test.rs"]
mod tests;

pub mod proxy_http;
pub mod session_persistence;

pub use conf_handler_impl::create_route_manager_handler;
pub use match_unit::HttpRouteRuleUnit;
pub use redirect_http::EdgionHttpRedirectProxy;
pub use routes_mgr::{
    get_global_http_route_managers, DomainRouteRules, GlobalHttpRouteManagers, HttpRouteManagerStats,
};

pub use proxy_http::EdgionHttpProxy;
