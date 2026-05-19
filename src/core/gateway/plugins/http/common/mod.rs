//! Common utilities shared across EdgionPlugins
//!
//! Provides shared infrastructure for plugins that need to make HTTP requests
//! to external services (e.g., ForwardAuth, OPA, Webhook), plus small helpers
//! reused across closely related plugins (e.g., the rate-limit family).

use std::time::Duration;

pub mod auth_common;
pub mod http_client;

pub use auth_common::{resolve_claim_path, send_gateway_auth_error_response, set_claims_headers, Claims};
pub use http_client::{
    connection_listed_hop_by_hop, get_http_client, is_hop_by_hop, is_hop_by_hop_or_listed, HOP_BY_HOP_HEADERS,
};

/// Format a duration in human-readable form (e.g., "1.5s", "500ms").
///
/// Shared by the `RateLimit` and `RateLimitRedis` plugins for the
/// `retry_in` response header.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_sub_second_uses_ms() {
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
    }

    #[test]
    fn format_duration_one_second_and_above_uses_seconds() {
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_secs(12)), "12.0s");
    }
}
