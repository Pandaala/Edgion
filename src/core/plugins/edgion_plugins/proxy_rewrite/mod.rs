//! Proxy Rewrite plugin module
//!
//! Rewrites requests before forwarding to upstream services.

mod plugin;

pub use plugin::ProxyRewrite;
