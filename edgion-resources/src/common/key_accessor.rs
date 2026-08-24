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
//!   skip:
//!     - type: keyExist
//!       key:
//!         type: header
//!         name: "X-Internal-Request"
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ============================================================================
// KeyGet - Read values from request context
// ============================================================================

/// Key accessor for reading values from request context
///
/// Used by RateLimit, PluginConditions, and other plugins that need
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
    /// Get a short string representation for logging
    pub fn as_log_str(&self) -> String {
        match self {
            KeyGet::RemoteIp => "remote-ip".to_string(),
            KeyGet::Header { name } => format!("hdr:{}", name),
            KeyGet::Cookie { name } => format!("cookie:{}", name),
            KeyGet::Query { name } => format!("query:{}", name),
            KeyGet::Path => "path".to_string(),
            KeyGet::Method => "method".to_string(),
            KeyGet::Ctx { name } => format!("ctx:{}", name),
            KeyGet::RemoteIpAndPath => "remote-ip+path".to_string(),
            KeyGet::RequestPath => "request-path".to_string(),
        }
    }

    /// Get the source type as a static string
    pub fn source_type(&self) -> &'static str {
        match self {
            KeyGet::RemoteIp => "remoteIp",
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
mod tests {
    use super::*;

    // ========== KeyGet Tests ==========

    #[test]
    fn test_key_get_as_log_str() {
        assert_eq!(KeyGet::RemoteIp.as_log_str(), "remote-ip");
        assert_eq!(
            KeyGet::Header {
                name: "X-Api-Key".to_string()
            }
            .as_log_str(),
            "hdr:X-Api-Key"
        );
        assert_eq!(KeyGet::Path.as_log_str(), "path");
        assert_eq!(KeyGet::RemoteIpAndPath.as_log_str(), "remote-ip+path");
        assert_eq!(KeyGet::RequestPath.as_log_str(), "request-path");
    }

    #[test]
    fn test_key_get_default() {
        assert_eq!(KeyGet::default(), KeyGet::RemoteIp);
    }

    #[test]
    fn test_key_get_source_type() {
        assert_eq!(KeyGet::RemoteIp.source_type(), "remoteIp");
        assert_eq!(
            KeyGet::Header {
                name: "X-Test".to_string()
            }
            .source_type(),
            "header"
        );
        assert_eq!(KeyGet::Path.source_type(), "path");
        assert_eq!(KeyGet::RequestPath.source_type(), "requestPath");
    }

    #[test]
    fn test_key_get_name() {
        assert_eq!(KeyGet::RemoteIp.name(), None);
        assert_eq!(
            KeyGet::Header {
                name: "X-Test".to_string()
            }
            .name(),
            Some("X-Test")
        );
        assert_eq!(
            KeyGet::Ctx {
                name: "user_id".to_string()
            }
            .name(),
            Some("user_id")
        );
    }

    // ========== KeySet Tests ==========

    #[test]
    fn test_key_set_as_log_str() {
        assert_eq!(
            KeySet::Header {
                name: "X-Test".to_string()
            }
            .as_log_str(),
            "hdr:X-Test"
        );
        assert_eq!(
            KeySet::ResponseHeader {
                name: "X-Time".to_string()
            }
            .as_log_str(),
            "res_hdr:X-Time"
        );
        assert_eq!(
            KeySet::Ctx {
                name: "user_id".to_string()
            }
            .as_log_str(),
            "ctx:user_id"
        );
    }

    #[test]
    fn test_key_set_name() {
        assert_eq!(
            KeySet::Header {
                name: "X-Test".to_string()
            }
            .name(),
            "X-Test"
        );
        assert_eq!(
            KeySet::Ctx {
                name: "user_id".to_string()
            }
            .name(),
            "user_id"
        );
    }

    // ========== Serde Tests ==========

    #[test]
    fn test_key_get_serde_header() {
        let key = KeyGet::Header {
            name: "X-Api-Key".to_string(),
        };

        let json = serde_json::to_string(&key).unwrap();
        assert!(json.contains("\"type\":\"header\""));
        assert!(json.contains("\"name\":\"X-Api-Key\""));

        let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, key);
    }

    #[test]
    fn test_key_get_serde_remote_ip() {
        let key = KeyGet::RemoteIp;
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "{\"type\":\"remoteIp\"}");

        let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, key);
    }

    #[test]
    fn test_key_get_serde_remote_ip_and_path() {
        let key = KeyGet::RemoteIpAndPath;
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "{\"type\":\"remoteIpAndPath\"}");

        let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, key);
    }

    #[test]
    fn test_key_get_serde_request_path() {
        let key = KeyGet::RequestPath;
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "{\"type\":\"requestPath\"}");

        let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, key);
    }

    #[test]
    fn test_key_set_serde() {
        let key = KeySet::ResponseHeader {
            name: "X-Response-Time".to_string(),
        };

        let json = serde_json::to_string(&key).unwrap();
        assert!(json.contains("\"type\":\"responseHeader\""));
        assert!(json.contains("\"name\":\"X-Response-Time\""));

        let deserialized: KeySet = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, key);
    }

    #[test]
    fn test_key_get_yaml_format() {
        let yaml = r#"
type: header
name: X-Api-Key
"#;
        let key: KeyGet = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            key,
            KeyGet::Header {
                name: "X-Api-Key".to_string()
            }
        );
    }

    #[test]
    fn test_key_get_yaml_no_name() {
        let yaml = r#"
type: remoteIp
"#;
        let key: KeyGet = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(key, KeyGet::RemoteIp);
    }

    #[test]
    fn old_client_ip_key_types_are_rejected() {
        for old in ["clientIp", "clientIpAndPath"] {
            let yaml = format!("type: {old}\n");
            assert!(serde_yaml::from_str::<KeyGet>(&yaml).is_err(), "{old} must be rejected");
        }
    }

    #[test]
    fn test_key_set_yaml_format() {
        let yaml = r#"
type: ctx
name: user_id
"#;
        let key: KeySet = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            key,
            KeySet::Ctx {
                name: "user_id".to_string()
            }
        );
    }
}
