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

use std::sync::OnceLock;

/// Global path normalization config, initialized once at startup.
static PATH_NORMALIZATION_CONFIG: OnceLock<PathNormalizationConfig> = OnceLock::new();

/// Initialize the global path normalization config (call once at startup).
pub fn init_path_normalization_config(config: PathNormalizationConfig) {
    let result = PATH_NORMALIZATION_CONFIG.set(config);
    debug_assert!(result.is_ok(), "init_path_normalization_config called more than once");
}

/// Get the global path normalization config (zero-cost after init).
///
/// Returns the default config (both reject flags off) if
/// `init_path_normalization_config` was not called (e.g., unit tests that
/// don't boot the full Gateway). Production paths always initialize before
/// the first request.
#[inline]
pub fn get_path_normalization_config() -> &'static PathNormalizationConfig {
    PATH_NORMALIZATION_CONFIG.get_or_init(PathNormalizationConfig::default)
}
