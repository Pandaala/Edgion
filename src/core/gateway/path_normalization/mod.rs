//! URI path normalization layer.
//!
//! Provides the Edgion BASE algorithm for normalizing HTTP request paths before
//! ACL evaluation and route matching.  The four steps are applied in fixed order:
//!
//! 1. Strip path-parameters (`/admin;jsessionid=x` → `/admin`)
//! 2. Percent-decode whitelisted characters (RFC 3986 unreserved + `%2F` + `%5C`)
//! 3. Merge consecutive slashes (`//` → `/`)
//! 4. Remove dot-segments (RFC 3986 §5.2.4)
//!
//! See `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §3.3 for
//! the full specification, security-commitment scope (§1.4), and configuration
//! options (`reject_escaped_slashes`, `reject_invalid_percent_encoding`).

pub mod config;
pub mod normalize;

pub use config::PathNormalizationConfig;
pub use normalize::{
    contains_escaped_slash, has_invalid_percent_encoding, needs_normalization, normalize_config_path, normalize_path,
};
