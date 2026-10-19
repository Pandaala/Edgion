//! Unified key accessor types for plugins
//!
//! This module provides unified types for accessing values from request context:
//! - `KeyGet`: Read values from headers, cookies, query params, etc.
//! - `KeySet`: Write values to headers, cookies, context variables, etc.
//!
//! ## Design Philosophy
//!
//! Parameters are embedded directly into enum variants for compile-time safety:
//! - `Header { name }` - name is required
//! - `RemoteIp` - no parameters needed
//!
//! ## YAML Configuration
//!
//! ```yaml
//! # Rate limiter key configuration
//! key:
//!   type: header
//!   name: "X-Api-Key"
//!
//! # Condition with key check
//! conditions:
//!   allOf:
//!     - type: keyExist
//!       key:
//!         type: header
//!         name: "X-Internal-Request"
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

// ============================================================================
// KeyGet - Read values from request context
// ============================================================================

/// Key accessor for reading values from request context
///
/// Used by RateLimitLocal, PluginConditions, and other plugins that need
/// to extract values from requests.
///
/// ## Usage
///
/// ```rust,ignore
/// let key = KeyGet::Header { name: "X-Api-Key".to_string() };
/// let value = key.get(&session);  // Returns Option<String>
/// ```
///
/// ## YAML Examples
///
/// ```yaml
/// # Get value from header
/// key:
///   type: header
///   name: "X-Api-Key"
///
/// # Get resolved remote IP (no name needed)
/// key:
///   type: remoteIp
///
/// # Get from cookie
/// key:
///   type: cookie
///   name: "session_id"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum KeyGet {
    /// Resolved remote IP address (after RealIp extraction)
    /// No additional parameters needed.
    #[default]
    RemoteIp,

    /// Physical downstream transport peer IP before PROXY/XFF rewriting.
    /// No additional parameters needed.
    DirectPeerIp,

    /// Standard W3C trace ID as 32 lowercase hexadecimal characters.
    TraceId,

    /// Normalized request hostname (lowercased, port-stripped, trailing FQDN dot removed).
    /// No additional parameters needed.
    Host,

    /// HTTP request header value
    /// Requires `name` to specify which header to read.
    Header {
        /// Header name (case-insensitive)
        name: String,
    },

    /// Cookie value
    /// Requires `name` to specify which cookie to read.
    Cookie {
        /// Cookie name
        name: String,
    },

    /// URL query parameter value
    /// Requires `name` to specify which query parameter to read.
    Query {
        /// Query parameter name
        name: String,
    },

    /// Request path (e.g., "/api/v1/users")
    /// No additional parameters needed.
    Path,

    /// HTTP method (GET, POST, etc.)
    /// No additional parameters needed.
    Method,

    /// Context variable (set by other plugins or system)
    /// Requires `name` to specify which context variable to read.
    Ctx {
        /// Context variable name
        name: String,
    },

    /// Combination of RemoteIp and Path
    /// Produces a key like "192.168.1.1:/api/users"
    /// No additional parameters needed.
    RemoteIpAndPath,

    /// Entry-time raw request path (never normalized).
    /// No additional parameters needed.
    RequestPath,
}

impl KeyGet {
    /// Get a short logging representation, borrowing fixed source names.
    pub fn as_log_str(&self) -> Cow<'static, str> {
        match self {
            KeyGet::RemoteIp => Cow::Borrowed("remote-ip"),
            KeyGet::DirectPeerIp => Cow::Borrowed("direct-peer-ip"),
            KeyGet::Host => Cow::Borrowed("host"),
            KeyGet::TraceId => Cow::Borrowed("trace-id"),
            KeyGet::Header { name } => Cow::Owned(format!("hdr:{}", name)),
            KeyGet::Cookie { name } => Cow::Owned(format!("cookie:{}", name)),
            KeyGet::Query { name } => Cow::Owned(format!("query:{}", name)),
            KeyGet::Path => Cow::Borrowed("path"),
            KeyGet::Method => Cow::Borrowed("method"),
            KeyGet::Ctx { name } => Cow::Owned(format!("ctx:{}", name)),
            KeyGet::RemoteIpAndPath => Cow::Borrowed("remote-ip+path"),
            KeyGet::RequestPath => Cow::Borrowed("request-path"),
        }
    }

    /// Get the source type as a static string
    pub fn source_type(&self) -> &'static str {
        match self {
            KeyGet::RemoteIp => "remoteIp",
            KeyGet::DirectPeerIp => "directPeerIp",
            KeyGet::Host => "host",
            KeyGet::TraceId => "traceId",
            KeyGet::Header { .. } => "header",
            KeyGet::Cookie { .. } => "cookie",
            KeyGet::Query { .. } => "query",
            KeyGet::Path => "path",
            KeyGet::Method => "method",
            KeyGet::Ctx { .. } => "ctx",
            KeyGet::RemoteIpAndPath => "remoteIpAndPath",
            KeyGet::RequestPath => "requestPath",
        }
    }

    /// Get the name parameter if applicable
    pub fn name(&self) -> Option<&str> {
        match self {
            KeyGet::Header { name } | KeyGet::Cookie { name } | KeyGet::Query { name } | KeyGet::Ctx { name } => {
                Some(name)
            }
            _ => None,
        }
    }
}

// ============================================================================
// KeySet - Write values to request/response context
// ============================================================================

/// Key accessor for writing values to request/response context
///
/// Used by plugins that need to modify headers, set cookies, or store
/// values in context variables.
///
/// ## YAML Examples
///
/// ```yaml
/// # Set request header
/// target:
///   type: header
///   name: "X-Request-ID"
///
/// # Set response header
/// target:
///   type: responseHeader
///   name: "X-Response-Time"
///
/// # Set context variable
/// target:
///   type: ctx
///   name: "user_id"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum KeySet {
    /// Set request header (before sending to upstream)
    Header {
        /// Header name
        name: String,
    },

    /// Set response header (before sending to client)
    ResponseHeader {
        /// Header name
        name: String,
    },

    /// Set cookie (in response)
    Cookie {
        /// Cookie name
        name: String,
    },

    /// Set context variable (for passing data between plugins)
    Ctx {
        /// Context variable name
        name: String,
    },
}

impl KeySet {
    /// Get a short string representation for logging
    pub fn as_log_str(&self) -> String {
        match self {
            KeySet::Header { name } => format!("hdr:{}", name),
            KeySet::ResponseHeader { name } => format!("res_hdr:{}", name),
            KeySet::Cookie { name } => format!("cookie:{}", name),
            KeySet::Ctx { name } => format!("ctx:{}", name),
        }
    }

    /// Get the name parameter
    pub fn name(&self) -> &str {
        match self {
            KeySet::Header { name }
            | KeySet::ResponseHeader { name }
            | KeySet::Cookie { name }
            | KeySet::Ctx { name } => name,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../tests/unit/common/key_accessor_test.rs"]
mod tests;
