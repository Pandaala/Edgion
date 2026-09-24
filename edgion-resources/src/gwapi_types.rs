//! Canonical Gateway API primitive-type grammars.
//!
//! Sibling of [`crate::duration`]: one shared constant per primitive type, so
//! the schemars attributes on resource structs and the hand-written CRD YAML
//! under `config/crd/edgion-crd/` can never drift from each other or from the
//! upstream Gateway API definitions.
//!
//! Patterns and bounds are copied verbatim from the committed upstream CRDs
//! (`config/crd/gateway-api/gateway-api-*-v1.6.2.yaml`); do not "improve" them
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
#[path = "../tests/unit/gwapi_types_test.rs"]
mod tests;
