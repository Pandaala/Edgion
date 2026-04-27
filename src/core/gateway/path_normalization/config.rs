//! Configuration for path normalization layer.

use serde::{Deserialize, Serialize};

/// Configuration for the URI path normalization middleware.
///
/// Both fields default to `false` so the normalization layer is non-blocking
/// by default: paths are still normalised (BASE algorithm) but unusual
/// encodings are forwarded rather than rejected.
///
/// ## TOML Configuration Example
///
/// ```toml
/// [path_normalization]
/// reject_escaped_slashes = true
/// reject_invalid_percent_encoding = true
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathNormalizationConfig {
    /// Reject any request whose path contains `%2F` or `%5C`. Default: false.
    ///
    /// Useful for high-security environments where path-delimiter encoding is
    /// never legitimate.  Beware: breaks legitimate APIs with base64 URL
    /// params containing `/`.
    pub reject_escaped_slashes: bool,

    /// Reject any request whose path contains malformed percent-encoding
    /// (invalid hex, or decoded byte sequence not valid UTF-8). Default: false.
    ///
    /// Captures IIS-style overlong UTF-8 (`%c0%ae`) and similar exotic inputs.
    pub reject_invalid_percent_encoding: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = PathNormalizationConfig::default();
        assert!(!cfg.reject_escaped_slashes);
        assert!(!cfg.reject_invalid_percent_encoding);
    }

    #[test]
    fn deserialize_from_toml() {
        let toml_str = r#"
            reject_escaped_slashes = true
            reject_invalid_percent_encoding = true
        "#;
        let cfg: PathNormalizationConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.reject_escaped_slashes);
        assert!(cfg.reject_invalid_percent_encoding);
    }

    #[test]
    fn deserialize_partial() {
        // Only one field set, the other defaults
        let toml_str = r#"reject_escaped_slashes = true"#;
        let cfg: PathNormalizationConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.reject_escaped_slashes);
        assert!(!cfg.reject_invalid_percent_encoding);
    }
}
