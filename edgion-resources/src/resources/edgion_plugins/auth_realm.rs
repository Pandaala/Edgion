//! Shared validation for authentication challenge realms and identity values.

/// Validate the unescaped realm before configuration is used or a challenge is built.
///
/// Quotes and backslashes are valid data and are escaped by the gateway's canonical
/// challenge writer. ASCII controls (including HTAB) and DEL are rejected rather
/// than stripped. Other UTF-8 bytes are preserved as HTTP quoted-string obs-text.
/// Existing plugin-specific empty-realm checks remain the caller's responsibility.
pub fn validate_auth_realm(realm: &str) -> Result<(), &'static str> {
    if realm.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("realm must not contain ASCII control characters (including tabs and DEL)");
    }
    Ok(())
}

/// Request headers no auth plugin may own as an identity output.
///
/// Owned outputs are removed on every continuation. Host is protocol-owned
/// (HTTP/2 authority can recreate it after sanitation); the others carry message
/// framing or connection semantics, so removing them would change how the
/// upstream parses the request rather than which identity it sees.
pub const RESERVED_AUTH_OUTPUT_HEADER_NAMES: &[&str] = &[
    "host",
    ":authority",
    "content-length",
    "transfer-encoding",
    "trailer",
    "te",
    "expect",
    "connection",
    "keep-alive",
    "proxy-connection",
    "upgrade",
];

/// Whether `name` is reserved and cannot be an auth plugin identity output.
pub fn is_reserved_auth_output_header(name: &str) -> bool {
    RESERVED_AUTH_OUTPUT_HEADER_NAMES
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

/// Validate the header names an auth plugin owns (clears and rewrites on every
/// continuation): each must be a valid header name of at most 256 bytes and not
/// a reserved request header. `field` is the configuration path for messages.
pub fn validate_owned_header_names(field: &str, names: &[String]) -> Result<(), String> {
    for (index, name) in names.iter().enumerate() {
        if name.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
            || !crate::gwapi_types::is_valid_header_token(name)
        {
            return Err(format!(
                "{field}[{index}] must be a valid HTTP header name of at most {} bytes",
                crate::gwapi_types::HEADER_NAME_MAX_LEN
            ));
        }
        if is_reserved_auth_output_header(name) {
            return Err(format!(
                "{field}[{index}] cannot be Host or a framing/hop-by-hop header; use a dedicated identity header"
            ));
        }
    }
    Ok(())
}

/// Admission message for an `anonymous` identity that cannot be written as a header.
pub const INVALID_ANONYMOUS_HEADER_TEXT: &str = "anonymous is not valid header text";

/// Validate an anonymous identity that an auth plugin writes as a request header.
///
/// The Controller and Gateway share this check: a value that is not header text
/// could never be forwarded and would deny every credential-less request.
pub fn validate_anonymous_header_text(anonymous: Option<&str>) -> Result<(), &'static str> {
    match anonymous {
        Some(anonymous) if http::HeaderValue::from_str(anonymous).is_err() => Err(INVALID_ANONYMOUS_HEADER_TEXT),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/auth_realm_test.rs"]
mod tests;
