//! Configuration for path normalization layer.

use serde::{Deserialize, Serialize};

/// Configuration for the URI path normalization middleware.
///
/// Defaults: paths are normalized (BASE algorithm), `;param` segments are stripped
/// (defends against `..;/` bypass attacks), and the normalized path is forwarded
/// to upstream. Unusual encodings are forwarded rather than rejected.
///
/// Default values:
/// - `reject_escaped_slashes`: `false` (forward `%2F` / `%5C` to upstream)
/// - `reject_invalid_percent_encoding`: `false` (forward malformed `%XX`)
/// - `preserve_path_parameters`: `false` (strip `;param` from each segment)
/// - `forward_normalized_path`: `true` (forward normalized path to upstream;
///    set to `false` for v3.1-compatible raw forwarding)
///
/// ## YAML Configuration Example
///
/// ```yaml
/// path_normalization:
///   reject_escaped_slashes: true
///   reject_invalid_percent_encoding: true
///   forward_normalized_path: false
///   preserve_path_parameters: true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Whether to preserve path-parameters (`;param=...`) literally.
    ///
    ///   - `false` (default) — strip path-parameters (matches v3.1 behavior; maintains
    ///     §1.4 security commitment scope including `..;/` bypass protection).
    ///   - `true` — preserve path-parameters (for upstreams that depend on `;jsessionid`
    ///     or other servlet path-parameter conventions).
    ///
    /// When set to `true`, `..;/` style bypass attacks are removed from the §1.4 default
    /// commitment scope (see spec §14.4).
    pub preserve_path_parameters: bool,

    /// Forward the normalized path to upstream. Default: true.
    ///
    /// When false, all routes default to forwarding the raw (un-normalized) path
    /// (matches v3.1 behavior; soft rollback channel for the v4 default change).
    /// Per-route override is available via annotation `edgion.io/forward-raw-path`.
    #[serde(default = "default_true")]
    pub forward_normalized_path: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PathNormalizationConfig {
    fn default() -> Self {
        Self {
            reject_escaped_slashes: false,
            reject_invalid_percent_encoding: false,
            preserve_path_parameters: false,
            forward_normalized_path: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = PathNormalizationConfig::default();
        assert!(!cfg.reject_escaped_slashes);
        assert!(!cfg.reject_invalid_percent_encoding);
        assert!(!cfg.preserve_path_parameters);
        assert!(cfg.forward_normalized_path);
    }

    #[test]
    fn deserialize_from_yaml() {
        let yaml_str = r#"
            reject_escaped_slashes: true
            reject_invalid_percent_encoding: true
        "#;
        let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert!(cfg.reject_escaped_slashes);
        assert!(cfg.reject_invalid_percent_encoding);
    }

    #[test]
    fn deserialize_partial() {
        // Only one field set, the other defaults
        let yaml_str = r#"reject_escaped_slashes: true"#;
        let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert!(cfg.reject_escaped_slashes);
        assert!(!cfg.reject_invalid_percent_encoding);
    }

    #[test]
    fn preserve_path_parameters_default_off() {
        let cfg = PathNormalizationConfig::default();
        assert!(!cfg.preserve_path_parameters);
    }

    #[test]
    fn deserialize_preserve_path_parameters() {
        let yaml_str = r#"preserve_path_parameters: true"#;
        let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert!(cfg.preserve_path_parameters);
    }

    #[test]
    fn forward_normalized_path_default_true() {
        let cfg = PathNormalizationConfig::default();
        assert!(cfg.forward_normalized_path);
    }

    #[test]
    fn deserialize_forward_normalized_path_false() {
        let yaml_str = r#"forward_normalized_path: false"#;
        let cfg: PathNormalizationConfig = serde_yaml::from_str(yaml_str).unwrap();
        assert!(!cfg.forward_normalized_path);
    }
}
