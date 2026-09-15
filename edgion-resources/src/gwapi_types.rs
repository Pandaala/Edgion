//! Canonical Gateway API primitive-type grammars.
//!
//! Sibling of [`crate::duration`]: one shared constant per primitive type, so
//! the schemars attributes on resource structs and the hand-written CRD YAML
//! under `config/crd/edgion-crd/` can never drift from each other or from the
//! upstream Gateway API definitions.
//!
//! Patterns and bounds are copied verbatim from the committed upstream CRDs
//! (`config/crd/gateway-api/gateway-api-*-v1.5.0.yaml`); do not "improve" them
//! here — mirror upstream. The drift guard
//! `edgion-resources/tests/crd_pattern_canonical.rs` asserts every inline
//! regex `pattern:` in the edgion CRDs equals one of these constants.
//! Numeric bounds (`minimum`/`maximum`/`maxLength`) in the CRD YAML are NOT
//! guarded — they are hand-synced with these constants by review.

/// Gateway API `PortNumber` lower bound (inclusive).
pub const PORT_MIN: i64 = 1;
/// Gateway API `PortNumber` upper bound (inclusive).
pub const PORT_MAX: i64 = 65535;

/// Gateway API `Hostname`: RFC 1123 DNS labels with an optional leading `*.`
/// wildcard label (route/listener hostnames).
pub const HOSTNAME_PATTERN: &str = r"^(\*\.)?[a-z0-9]([-a-z0-9]*[a-z0-9])?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$";

/// Gateway API `PreciseHostname`: same as [`HOSTNAME_PATTERN`] but with no
/// wildcard label (redirect/rewrite hostname, backend TLS SNI).
pub const PRECISE_HOSTNAME_PATTERN: &str = r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$";

/// Maximum hostname length (both hostname forms).
pub const HOSTNAME_MAX_LEN: u32 = 253;

/// Gateway API `HeaderName`: RFC 7230 header field-name token.
pub const HEADER_NAME_PATTERN: &str = r"^[A-Za-z0-9!#$%&'*+\-.^_`|~]+$";

/// Maximum header name length.
pub const HEADER_NAME_MAX_LEN: u32 = 256;

/// Canonical static hop-by-hop header names from RFC 9110 section 7.6.1.
///
/// The per-message names nominated by `Connection` cannot be known during
/// configuration validation and must be handled by the forwarding path.
pub const HTTP_HOP_BY_HOP_HEADER_NAMES: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Default upstream header carrying the final resolved remote IP.
pub const DEFAULT_REMOTE_IP_HEADER: &str = "X-Real-IP";

/// Additional names reserved only for the configured RemoteIp write target.
const REMOTE_IP_ONLY_RESERVED_HEADER_NAMES: [&str; 3] = ["host", "content-length", "x-forwarded-for"];

/// Kubernetes label-value charset used by Edgion named-group references
/// (KeyAuth `secretGroups[].name`, JwtAuth `issuerGroups[].name`, ...).
/// Runtime twin: [`crate::resources::common::group_name::validate_group_name`].
pub const GROUP_NAME_PATTERN: &str = r"^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$";

/// Maximum group name length (Kubernetes label-value limit).
pub const GROUP_NAME_MAX_LEN: u32 = 63;

/// Runtime check for [`PORT_MIN`]`..=`[`PORT_MAX`] — for validators guarding
/// fields whose CRD schema is not admission-enforced (plugin configs behind
/// `x-kubernetes-preserve-unknown-fields`, FS-mode ingest).
pub fn is_valid_port(port: i64) -> bool {
    (PORT_MIN..=PORT_MAX).contains(&port)
}

/// Runtime twin of [`HEADER_NAME_PATTERN`]: RFC 7230/9110 header field-name
/// `token` charset check. Kept equivalent by
/// `tests::header_token_fn_matches_pattern`.
pub fn is_valid_header_token(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            matches!(b,
                b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~'
                | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
        })
}

/// Whether `name` is a canonical static hop-by-hop header name.
pub fn is_http_hop_by_hop_header_name(name: &str) -> bool {
    HTTP_HOP_BY_HOP_HEADER_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

/// Whether `name` is owned by HTTP message framing or hop-by-hop handling.
pub fn is_http_framing_or_hop_by_hop_header_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length") || is_http_hop_by_hop_header_name(name)
}

/// Validate an operator-configured upstream header that carries `RemoteIp`.
///
/// The scalar remote-IP value must never overwrite framing, hop-by-hop, or the
/// Gateway-managed `X-Forwarded-For` chain.
pub fn validate_remote_ip_header_name(name: &str) -> Result<(), String> {
    if name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(name) {
        return Err("forwardedHeaders.remoteIpHeader must be a valid HTTP header name".to_string());
    }
    if is_http_hop_by_hop_header_name(name)
        || REMOTE_IP_ONLY_RESERVED_HEADER_NAMES
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
    {
        return Err(format!("forwardedHeaders.remoteIpHeader '{name}' is reserved"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn matches(pattern: &str, s: &str) -> bool {
        Regex::new(pattern).unwrap().is_match(s)
    }

    #[test]
    fn hostname_pattern() {
        for ok in ["example.com", "*.example.com", "a", "svc.ns.svc.cluster.local"] {
            assert!(matches(HOSTNAME_PATTERN, ok), "should accept {ok:?}");
        }
        for bad in [
            "",
            "Example.com",
            "*.",
            "-foo.com",
            "foo-.com",
            "*.*.example.com",
            "foo..com",
        ] {
            assert!(!matches(HOSTNAME_PATTERN, bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn precise_hostname_pattern() {
        for ok in ["example.com", "a", "api.v1.example.com"] {
            assert!(matches(PRECISE_HOSTNAME_PATTERN, ok), "should accept {ok:?}");
        }
        for bad in ["", "*.example.com", "Example.com", "foo_bar.com"] {
            assert!(!matches(PRECISE_HOSTNAME_PATTERN, bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn header_name_pattern() {
        for ok in ["Content-Type", "x-request-id", "X_Custom.Header", "etag"] {
            assert!(matches(HEADER_NAME_PATTERN, ok), "should accept {ok:?}");
        }
        for bad in ["", "content type", "header:name", "héader", "head(er)"] {
            assert!(!matches(HEADER_NAME_PATTERN, bad), "should reject {bad:?}");
        }
    }

    /// The schema-side pattern and the runtime charset check must accept/reject
    /// the same header names.
    #[test]
    fn header_token_fn_matches_pattern() {
        let re = Regex::new(HEADER_NAME_PATTERN).unwrap();
        for s in [
            "Content-Type",
            "x-request-id",
            "X_Custom.Header",
            "`|~!",
            "",
            "a b",
            "a:b",
            "héader",
        ] {
            assert_eq!(
                re.is_match(s),
                is_valid_header_token(s),
                "pattern and is_valid_header_token disagree on {s:?}"
            );
        }
    }

    #[test]
    fn remote_ip_header_validation_rejects_reserved_names() {
        assert!(validate_remote_ip_header_name(DEFAULT_REMOTE_IP_HEADER).is_ok());
        assert!(validate_remote_ip_header_name("X-Edgion-Remote-IP").is_ok());
        assert!(validate_remote_ip_header_name("bad header").is_err());
        assert!(validate_remote_ip_header_name(&"x".repeat(HEADER_NAME_MAX_LEN as usize + 1)).is_err());
        for reserved in HTTP_HOP_BY_HOP_HEADER_NAMES
            .iter()
            .copied()
            .chain(REMOTE_IP_ONLY_RESERVED_HEADER_NAMES)
        {
            assert!(
                validate_remote_ip_header_name(reserved).is_err(),
                "{reserved} must be reserved"
            );
        }
    }

    #[test]
    fn framing_and_hop_by_hop_header_detection_is_case_insensitive() {
        assert!(is_http_framing_or_hop_by_hop_header_name("Content-Length"));
        for name in HTTP_HOP_BY_HOP_HEADER_NAMES {
            assert!(is_http_hop_by_hop_header_name(name), "{name}");
            assert!(is_http_framing_or_hop_by_hop_header_name(&name.to_ascii_uppercase()));
        }
        assert!(!is_http_framing_or_hop_by_hop_header_name("x-rate-limit-limit"));
    }

    #[test]
    fn port_bounds() {
        assert!(!is_valid_port(0));
        assert!(is_valid_port(1));
        assert!(is_valid_port(65535));
        assert!(!is_valid_port(65536));
        assert!(!is_valid_port(-1));
    }

    /// The schemars attributes on resource structs must actually emit the
    /// constraint keywords — a silently ignored attribute would leave the
    /// generated schema unconstrained with no compile error. Spot-checks one
    /// representative field per attribute shape (scalar `range`, scalar
    /// `pattern`, and array `inner(...)`).
    #[test]
    fn schemars_attributes_emit_constraints() {
        use schemars::schema_for;

        // Scalar range on Option<i32>: ParentReference.port.
        let schema = serde_json::to_value(schema_for!(crate::resources::common::ParentReference)).unwrap();
        let port = &schema["properties"]["port"];
        assert_eq!(
            port["minimum"],
            serde_json::json!(PORT_MIN),
            "port minimum missing: {port}"
        );
        assert_eq!(
            port["maximum"],
            serde_json::json!(PORT_MAX),
            "port maximum missing: {port}"
        );

        // inner(pattern + length) on Vec<String>: EdgionTlsSpec.hosts.
        let schema = serde_json::to_value(schema_for!(crate::resources::edgion_tls::EdgionTlsSpec)).unwrap();
        let items = &schema["properties"]["hosts"]["items"];
        assert_eq!(
            items["pattern"],
            serde_json::json!(HOSTNAME_PATTERN),
            "hosts items: {items}"
        );
        assert_eq!(
            items["maxLength"],
            serde_json::json!(HOSTNAME_MAX_LEN),
            "hosts items: {items}"
        );

        // inner(...) must also propagate through Option<Vec<String>>:
        // HTTPRouteSpec.hostnames (schemars wraps Option as anyOf[T, null]).
        let schema = serde_json::to_value(schema_for!(crate::resources::http_route::HTTPRouteSpec)).unwrap();
        let hostnames = &schema["properties"]["hostnames"];
        let arr = hostnames["anyOf"]
            .as_array()
            .and_then(|v| v.iter().find(|m| m["type"] == "array"))
            .unwrap_or(hostnames);
        let items = &arr["items"];
        assert_eq!(
            items["pattern"],
            serde_json::json!(HOSTNAME_PATTERN),
            "route hostnames items: {hostnames}"
        );
    }

    /// The schema-side pattern and the runtime validator must accept/reject
    /// the same names (charset only — the 63-char limit is `maxLength`'s job,
    /// enforced separately via [`GROUP_NAME_MAX_LEN`]).
    #[test]
    fn group_name_pattern_matches_runtime_validator() {
        use crate::resources::common::group_name::validate_group_name;
        let re = Regex::new(GROUP_NAME_PATTERN).unwrap();
        for s in [
            "a",
            "0",
            "partner-a",
            "internal_keys",
            "v1.0",
            "",
            "-foo",
            "foo.",
            "foo bar",
            "foo/bar",
            "a-b_c",
        ] {
            assert_eq!(
                re.is_match(s),
                validate_group_name(s).is_ok(),
                "pattern and validate_group_name disagree on {s:?}"
            );
        }
    }
}
