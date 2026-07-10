//! Shared dye-header (traffic tagging) configuration.
//!
//! Selection-driven plugins (Canary, DynamicInternalUpstream,
//! DynamicExternalUpstream, DirectEndpoint, RegionRoute) embed this struct to
//! expose their per-request selection decision as HTTP headers. See
//! `docs/superpowers/specs/2026-07-10-dye-headers-unification-design.md`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Header names whose overwrite would corrupt message framing or bypass
/// routing machinery (e.g. a request-side `Host` dye would silently clobber
/// `set_upstream_host`/`override_host` handling). Checked case-insensitively,
/// in both directions.
const DENIED_HEADER_NAMES: [&str; 7] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "te",
    "upgrade",
    "trailer",
];

/// Traffic-tagging ("dye") headers shared by selection-driven plugins.
/// Each entry sets one header; `value: None` means "use the plugin's
/// resolved selection value" (profile name, target name, endpoint addr, ...).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DyeHeaders {
    /// Headers set on the upstream request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request: Vec<DyeHeader>,
    /// Headers set on the downstream response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response: Vec<DyeHeader>,
}

/// One dye header entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DyeHeader {
    /// Header name (must be a valid HTTP field-name token).
    pub name: String,
    /// Static header value. Omitted => the resolved selection value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// RFC 9110 `token` charset check for header names.
fn is_valid_token(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            matches!(b,
                b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~'
                | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
        })
}

/// HTTP field-value byte check: no control bytes (CR/LF/NUL/...), tab allowed.
fn is_valid_field_value(value: &str) -> bool {
    value.bytes().all(|b| b == b'\t' || b >= 0x20 && b != 0x7f)
}

impl DyeHeaders {
    /// Stateless validation. Called from each host plugin's
    /// `PluginValidation::validation_error`, so bad dye config is rejected at
    /// controller ingest (Accepted=False) and at gateway compile alike.
    pub fn validation_error(&self) -> Option<String> {
        for (direction, entries) in [("request", &self.request), ("response", &self.response)] {
            for entry in entries {
                if !is_valid_token(&entry.name) {
                    return Some(format!(
                        "dyeHeaders.{}: invalid header name '{}'",
                        direction, entry.name
                    ));
                }
                if DENIED_HEADER_NAMES.contains(&entry.name.to_ascii_lowercase().as_str()) {
                    return Some(format!(
                        "dyeHeaders.{}: header '{}' is not allowed as a dye header",
                        direction, entry.name
                    ));
                }
                if let Some(ref value) = entry.value {
                    if !is_valid_field_value(value) {
                        return Some(format!(
                            "dyeHeaders.{}: invalid value for header '{}'",
                            direction, entry.name
                        ));
                    }
                }
            }
        }
        None
    }

    /// Reject a request-direction dye name equal (case-insensitively) to one
    /// of the host plugin's own header-typed key sources. The entry-point
    /// anti-spoof strip runs before key extraction, so such a config would
    /// deterministically remove the plugin's own routing input and force the
    /// on-missing path on every request (silent misroute or full reject),
    /// with nothing in the access log pointing at the collision.
    ///
    /// `key_header_names` is the host's list of header names it reads as
    /// routing keys (`KeyGet::Header` sources only — other source types
    /// cannot collide with a request header).
    pub fn key_collision_error(&self, key_header_names: &[&str]) -> Option<String> {
        for entry in &self.request {
            if key_header_names.iter().any(|k| k.eq_ignore_ascii_case(&entry.name)) {
                return Some(format!(
                    "dyeHeaders.request: header '{}' collides with the plugin's own \
                     header key source; the anti-spoof strip would remove the routing \
                     key before extraction",
                    entry.name
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, value: Option<&str>) -> DyeHeader {
        DyeHeader {
            name: name.to_string(),
            value: value.map(|v| v.to_string()),
        }
    }

    #[test]
    fn empty_config_is_valid() {
        assert!(DyeHeaders::default().validation_error().is_none());
    }

    #[test]
    fn key_collision_is_rejected_case_insensitively() {
        let cfg = DyeHeaders {
            request: vec![entry("X-Backend-Target", None)],
            response: vec![],
        };
        let err = cfg.key_collision_error(&["x-backend-target"]).unwrap();
        assert!(err.contains("collides"), "got: {}", err);
        assert!(err.contains("X-Backend-Target"), "got: {}", err);
    }

    #[test]
    fn response_direction_never_collides_with_key_sources() {
        // Only request-direction dye names are stripped from the incoming
        // request, so a response entry sharing a key-source name is fine.
        let cfg = DyeHeaders {
            request: vec![],
            response: vec![entry("X-Backend-Target", None)],
        };
        assert!(cfg.key_collision_error(&["X-Backend-Target"]).is_none());
    }

    #[test]
    fn distinct_names_and_empty_key_list_pass() {
        let cfg = DyeHeaders {
            request: vec![entry("X-Release-Track", None)],
            response: vec![],
        };
        assert!(cfg.key_collision_error(&["X-Backend-Target"]).is_none());
        assert!(cfg.key_collision_error(&[]).is_none());
    }

    #[test]
    fn typical_config_is_valid() {
        let cfg = DyeHeaders {
            request: vec![entry("X-Release-Track", None)],
            response: vec![entry("X-Served-By", Some("edge-cluster-1"))],
        };
        assert!(cfg.validation_error().is_none());
    }

    #[test]
    fn invalid_token_name_is_rejected() {
        let cfg = DyeHeaders {
            request: vec![entry("X Bad Name", None)],
            response: vec![],
        };
        let err = cfg.validation_error().unwrap();
        assert!(err.contains("invalid header name"), "got: {}", err);
        assert!(err.contains("request"), "direction missing: {}", err);
    }

    #[test]
    fn empty_name_is_rejected() {
        let cfg = DyeHeaders {
            request: vec![entry("", None)],
            response: vec![],
        };
        assert!(cfg.validation_error().is_some());
    }

    #[test]
    fn denied_names_are_rejected_case_insensitively_in_both_directions() {
        for name in [
            "Host",
            "content-length",
            "Transfer-Encoding",
            "Connection",
            "TE",
            "Upgrade",
            "Trailer",
        ] {
            let req = DyeHeaders {
                request: vec![entry(name, Some("x"))],
                response: vec![],
            };
            assert!(req.validation_error().is_some(), "request '{}' must be denied", name);
            let resp = DyeHeaders {
                request: vec![],
                response: vec![entry(name, Some("x"))],
            };
            assert!(resp.validation_error().is_some(), "response '{}' must be denied", name);
        }
    }

    #[test]
    fn control_bytes_in_value_are_rejected() {
        for bad in ["evil\r\nInjected: 1", "nul\u{0}byte", "del\u{7f}char"] {
            let cfg = DyeHeaders {
                request: vec![entry("X-Ok-Name", Some(bad))],
                response: vec![],
            };
            assert!(cfg.validation_error().is_some(), "value {:?} must be rejected", bad);
        }
    }

    #[test]
    fn tab_and_space_in_value_are_allowed() {
        let cfg = DyeHeaders {
            request: vec![entry("X-Ok-Name", Some("a b\tc"))],
            response: vec![],
        };
        assert!(cfg.validation_error().is_none());
    }

    #[test]
    fn serde_camel_case_round_trip_with_omitted_value() {
        let yaml = "request:\n  - name: X-Release-Track\nresponse:\n  - name: X-Served-By\n    value: edge-1\n";
        let cfg: DyeHeaders = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.request.len(), 1);
        assert_eq!(cfg.request[0].name, "X-Release-Track");
        assert!(cfg.request[0].value.is_none());
        assert_eq!(cfg.response[0].value.as_deref(), Some("edge-1"));

        let json = serde_json::to_string(&cfg).unwrap();
        let back: DyeHeaders = serde_json::from_str(&json).unwrap();
        assert_eq!(back.response[0].name, "X-Served-By");
    }
}
