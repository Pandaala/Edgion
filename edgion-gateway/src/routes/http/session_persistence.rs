//! Session Persistence utilities for HTTP routes.
//!
//! Provides token generation, cookie/header extraction, and Set-Cookie building
//! for Gateway API SessionPersistence support.

use edgion_resources::resources::http_route::{CookieLifetimeType, SessionPersistence, SessionPersistenceType};
use pingora_proxy::Session;

/// Default session persistence cookie/header name.
const DEFAULT_SESSION_NAME: &str = "edgion-session-affinity";

/// Generate a session token from a backend address (ip:port).
///
/// Returns a deterministic 8-character lowercase hex string (CRC32 hash).
///
/// Threat model: this token is a stickiness/performance hint, NOT a security
/// boundary. It is a plain CRC32 of `IP:port` with no per-process secret and no
/// MAC, so a client that knows a backend's `IP:port` can pre-compute the token
/// and forge a cookie/header to pin its OWN requests to a chosen backend. This
/// is self-affecting only: the token identifies a backend (not a user session),
/// the affinity map is built solely from the service's real backends so a forged
/// token can only select an existing backend (never inject an arbitrary address),
/// and the resolved sticky target still passes SSRF validation on the upstream-peer
/// path (`get_peer_with_sp_fallback` / `try_get_peer`). Do NOT repurpose session
/// affinity as access
/// control; if routing restrictions are ever required (e.g. tenant isolation by
/// pod), replace CRC32 with an HMAC over a per-process/service secret.
pub fn generate_session_token(backend_addr: &str) -> String {
    let hash = crc32fast::hash(backend_addr.as_bytes());
    format!("{:08x}", hash)
}

/// Get the session name from a `SessionPersistence` config, falling back to the default.
pub fn get_session_name(sp: &SessionPersistence) -> &str {
    sp.session_name.as_deref().unwrap_or(DEFAULT_SESSION_NAME)
}

/// Get the persistence type, defaulting to Cookie.
pub fn get_persistence_type(sp: &SessionPersistence) -> SessionPersistenceType {
    sp.persistence_type.clone().unwrap_or(SessionPersistenceType::Cookie)
}

/// Extract a session token from the Cookie header.
pub fn extract_token_from_cookie(session: &Session, cookie_name: &str) -> Option<String> {
    let req_header = session.req_header();
    req_header
        .headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').map(|s| s.trim()).find_map(|cookie| {
                let (name, value) = cookie.split_once('=')?;
                if name.trim() == cookie_name {
                    Some(value.trim().to_string())
                } else {
                    None
                }
            })
        })
}

/// Extract a session token from a custom header.
pub fn extract_token_from_header(session: &Session, header_name: &str) -> Option<String> {
    let req_header = session.req_header();
    req_header
        .headers
        .get(header_name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Unified extraction: uses cookie or header based on the persistence type config.
pub fn extract_session_token(session: &Session, sp: &SessionPersistence) -> Option<String> {
    let name = get_session_name(sp);
    match get_persistence_type(sp) {
        SessionPersistenceType::Cookie => extract_token_from_cookie(session, name),
        SessionPersistenceType::Header => extract_token_from_header(session, name),
    }
}

/// Build a Set-Cookie header value for session persistence.
///
/// Format: `name=value; Path=/[; Max-Age=N]; HttpOnly; SameSite=Lax`
/// Max-Age is only included for Permanent lifetime using absolute_timeout.
pub fn build_set_cookie_value(sp: &SessionPersistence, token: &str, is_tls: bool) -> String {
    let name = get_session_name(sp);
    let mut cookie = format!("{}={}; Path=/", name, token);

    // Add Max-Age only for Permanent lifetime
    let is_permanent = sp
        .cookie_config
        .as_ref()
        .and_then(|c| c.lifetime_type.as_ref())
        .map(|lt| *lt == CookieLifetimeType::Permanent)
        .unwrap_or(false);

    if is_permanent {
        if let Some(ref timeout) = sp.absolute_timeout {
            if let Some(seconds) = parse_duration_to_seconds(timeout) {
                cookie.push_str(&format!("; Max-Age={}", seconds));
            }
        }
    }

    cookie.push_str("; HttpOnly; SameSite=Lax");
    if is_tls {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Parse a duration string to seconds.
///
/// Supports formats: "1h", "30m", "60s", or raw seconds (e.g. "3600").
pub fn parse_duration_to_seconds(duration: &str) -> Option<u64> {
    let duration = duration.trim();
    if duration.is_empty() {
        return None;
    }

    if let Some(hours) = duration.strip_suffix('h') {
        hours.trim().parse::<u64>().ok().map(|h| h * 3600)
    } else if let Some(minutes) = duration.strip_suffix('m') {
        minutes.trim().parse::<u64>().ok().map(|m| m * 60)
    } else if let Some(seconds) = duration.strip_suffix('s') {
        seconds.trim().parse::<u64>().ok()
    } else {
        duration.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::http_route::CookieConfig;

    #[test]
    fn test_token_generation_determinism() {
        let addr = "10.0.0.1:8080";
        let t1 = generate_session_token(addr);
        let t2 = generate_session_token(addr);
        assert_eq!(t1, t2, "token generation must be deterministic");
        assert_eq!(t1.len(), 8, "token must be 8 hex chars");
    }

    #[test]
    fn test_token_generation_different_addrs() {
        let t1 = generate_session_token("10.0.0.1:8080");
        let t2 = generate_session_token("10.0.0.2:8080");
        assert_ne!(t1, t2, "different addresses should produce different tokens");
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration_to_seconds("1h"), Some(3600));
        assert_eq!(parse_duration_to_seconds("24h"), Some(86400));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration_to_seconds("30m"), Some(1800));
        assert_eq!(parse_duration_to_seconds("1m"), Some(60));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration_to_seconds("60s"), Some(60));
        assert_eq!(parse_duration_to_seconds("3600"), Some(3600));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration_to_seconds(""), None);
        assert_eq!(parse_duration_to_seconds("abc"), None);
    }

    #[test]
    fn test_default_session_name() {
        let sp = SessionPersistence {
            session_name: None,
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        };
        assert_eq!(get_session_name(&sp), "edgion-session-affinity");
    }

    #[test]
    fn test_custom_session_name() {
        let sp = SessionPersistence {
            session_name: Some("my-session".to_string()),
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        };
        assert_eq!(get_session_name(&sp), "my-session");
    }

    #[test]
    fn test_default_persistence_type() {
        let sp = SessionPersistence {
            session_name: None,
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        };
        assert_eq!(get_persistence_type(&sp), SessionPersistenceType::Cookie);
    }

    #[test]
    fn test_header_persistence_type() {
        let sp = SessionPersistence {
            session_name: None,
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: Some(SessionPersistenceType::Header),
            cookie_config: None,
            strict: None,
        };
        assert_eq!(get_persistence_type(&sp), SessionPersistenceType::Header);
    }

    #[test]
    fn test_build_set_cookie_session_lifetime() {
        let sp = SessionPersistence {
            session_name: None,
            absolute_timeout: Some("1h".to_string()),
            idle_timeout: None,
            persistence_type: None,
            cookie_config: Some(CookieConfig {
                lifetime_type: Some(CookieLifetimeType::Session),
            }),
            strict: None,
        };
        let cookie = build_set_cookie_value(&sp, "abcd1234", false);
        assert_eq!(
            cookie,
            "edgion-session-affinity=abcd1234; Path=/; HttpOnly; SameSite=Lax"
        );
        assert!(!cookie.contains("Max-Age"), "Session lifetime should not have Max-Age");
    }

    #[test]
    fn test_build_set_cookie_permanent_lifetime() {
        let sp = SessionPersistence {
            session_name: Some("my-sess".to_string()),
            absolute_timeout: Some("24h".to_string()),
            idle_timeout: None,
            persistence_type: None,
            cookie_config: Some(CookieConfig {
                lifetime_type: Some(CookieLifetimeType::Permanent),
            }),
            strict: None,
        };
        let cookie = build_set_cookie_value(&sp, "abcd1234", false);
        assert_eq!(
            cookie,
            "my-sess=abcd1234; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax"
        );
    }

    #[test]
    fn test_build_set_cookie_no_cookie_config() {
        let sp = SessionPersistence {
            session_name: None,
            absolute_timeout: Some("1h".to_string()),
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        };
        let cookie = build_set_cookie_value(&sp, "token123", false);
        // No cookie_config means not permanent, so no Max-Age
        assert!(!cookie.contains("Max-Age"));
    }

    #[test]
    fn test_build_set_cookie_secure_flag_when_tls() {
        let sp = SessionPersistence {
            session_name: None,
            absolute_timeout: None,
            idle_timeout: None,
            persistence_type: None,
            cookie_config: None,
            strict: None,
        };
        let cookie = build_set_cookie_value(&sp, "abcd1234", true);
        assert_eq!(
            cookie,
            "edgion-session-affinity=abcd1234; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
    }
}
