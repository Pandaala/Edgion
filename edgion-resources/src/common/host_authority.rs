//! Shared HTTP Host authority syntax validation.

use http::uri::Authority;

/// Validate a non-empty Host authority with no userinfo and an optional port in 1..=65535.
pub fn is_valid_host_authority(value: &str) -> bool {
    // HTTP Host is `uri-host [ ":" port ]`. URI Authority also allows userinfo.
    if value.is_empty() || value.contains('@') {
        return false;
    }
    let Ok(authority) = Authority::try_from(value) else {
        return false;
    };
    let host = authority.host();
    if host.is_empty() {
        return false;
    }
    if value == host {
        return true;
    }
    let Some(port) = value.get(host.len()..).and_then(|rest| rest.strip_prefix(':')) else {
        return false;
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    port.parse::<i64>().ok().is_some_and(crate::gwapi_types::is_valid_port)
}
