//! Request Restriction plugin module
//!
//! This plugin restricts access based on request attributes like headers, cookies,
//! query parameters, path, method, and referer.

mod plugin;

pub use plugin::RequestRestriction;
