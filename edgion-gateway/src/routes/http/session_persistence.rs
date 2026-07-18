//! Session Persistence utilities for HTTP routes.
//!
//! Provides token generation, cookie/header extraction, and Set-Cookie building
//! for Gateway API SessionPersistence support.

use edgion_resources::resources::http_route::{CookieLifetimeType, SessionPersistence, SessionPersistenceType};
use pingora_proxy::Session;
use sha2::{Digest, Sha256};

use crate::{EdgionHttpContext, SessionPersistenceState};

/// Default session persistence cookie/header name.
const DEFAULT_SESSION_NAME: &str = "edgion-session-affinity";

/// Generate a session token from a backend address (ip:port).
///
/// Returns a deterministic 16-character lowercase hex string (SHA-256
/// truncated to 64 bits). 64 bits keeps the accidental-collision probability
/// negligible now that a token is resolved across ALL of a rule's backendRefs
/// (a 32-bit CRC was deemed too narrow for a cross-Service identity); a
/// one-way hash (rather than an Envoy-style decodable `base64(ip:port)`) is
/// deliberate so backend pod addresses are never exposed to clients.
///
/// Threat model: this token is a stickiness/performance hint, NOT a security
/// boundary. It is an unkeyed hash of `IP:port` with no per-process secret and
/// no MAC, so a client that knows a backend's `IP:port` can pre-compute the
/// token and forge a cookie/header to pin its OWN requests to a chosen backend.
/// This is self-affecting only: the token identifies a backend (not a user
/// session), the affinity map is built solely from the service's real backends
/// so a forged token can only select an existing backend (never inject an
/// arbitrary address), and the resolved sticky target still passes SSRF
/// validation on the upstream-peer path (`get_peer_with_sp_fallback` /
/// `try_get_peer`). Do NOT repurpose session affinity as access control; if
/// routing restrictions are ever required (e.g. tenant isolation by pod),
/// replace the unkeyed hash with an HMAC over a per-process/service secret.
pub fn generate_session_token(backend_addr: &str) -> String {
    let digest = Sha256::digest(backend_addr.as_bytes());
    let prefix: [u8; 8] = digest[..8].try_into().expect("SHA-256 digest is 32 bytes");
    format!("{:016x}", u64::from_be_bytes(prefix))
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

/// Session Persistence step 1 of 2: extract the session token (if SP is
/// configured on the effective rule) and pin the initial state.
/// Protocol-neutral: callers pass the rule's `session_persistence` regardless
/// of HTTPRoute or GRPCRoute origin.
///
/// Takes `&mut SessionPersistenceState` rather than `&mut EdgionHttpContext`
/// to keep the borrow disjoint from the caller's live route-unit borrow.
///
/// - SP not configured → returns `None`, state untouched.
/// - SP configured, token present → state = `Inactive` (a fallback later may flip it).
/// - SP configured, no token → state = `InjectNew` (first request; the response injects one).
#[inline]
pub(crate) fn sp_extract_token(
    session: &Session,
    sp_state: &mut SessionPersistenceState,
    sp: Option<&SessionPersistence>,
) -> Option<String> {
    let sp = sp?;
    let token = extract_session_token(session, sp);
    *sp_state = if token.is_some() {
        SessionPersistenceState::Inactive
    } else {
        SessionPersistenceState::InjectNew
    };
    token
}

/// Session Persistence step 2 of 2: resolve a token to a sticky
/// (backendRef, endpoint) pair across ALL of the matched rule's backendRefs,
/// BEFORE weighted backendRef selection. Protocol-neutral: callers pass the
/// rule's backendRef `(name, namespace)` pairs in declaration order, which
/// both HTTP and gRPC compiled backends provide.
///
/// GEP-1619: weights govern only sessions without a token; an established
/// session MUST keep routing to its original Service and endpoint even when
/// that backendRef's weight has since changed (including to zero). Callers
/// therefore iterate the rule's FULL backendRef list (weight-0 refs included,
/// unlike the WRR selector which filters them out).
///
/// Rule scoping is structural: the walk only visits the matched rule's own
/// backendRefs, so two rules sharing the default session name cannot adopt
/// each other's sessions unless they reference the same Service — in which
/// case both resolve to the same pod, matching Envoy Gateway semantics.
///
/// - Token absent → `None`, state untouched (step 1 already set `InjectNew`).
/// - Token hits some backendRef's Service affinity map → set `sp_target_addr`
///   so `try_get_peer` routes directly to it; return that backendRef's index
///   so the caller selects the SAME backendRef (TLS policy + backend plugins
///   must follow the sticky Service, not a fresh WRR pick).
/// - Token misses every backendRef (endpoint or backendRef gone) → flip state
///   to `InjectNew` so the caller falls back to weighted selection and the
///   response refreshes the token.
pub(crate) fn sp_resolve_target_rule_scoped<'a>(
    ctx: &mut EdgionHttpContext,
    backends: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    route_ns: &str,
    token: Option<&str>,
) -> Option<usize> {
    let token = token?;
    match sp_lookup_rule_scoped(backends, route_ns, token) {
        Some((idx, addr)) => {
            ctx.sp_target_addr = Some(addr);
            ctx.sp_sticky_addr = Some(addr);
            Some(idx)
        }
        None => {
            ctx.sp_state = SessionPersistenceState::InjectNew;
            None
        }
    }
}

/// Pure rule-scoped lookup: walk the backendRef `(name, namespace)` pairs in
/// declaration order and return the first whose Service affinity map contains
/// `token`, together with the resolved endpoint address.
///
/// Declaration order makes the walk deterministic across gateway replicas. If
/// the same endpoint address is registered under two backendRefs' Services
/// (two Services selecting the same pods on the same targetPort), the first
/// declared backendRef wins — the same ambiguity a merged Envoy cluster has.
fn sp_lookup_rule_scoped<'a>(
    backends: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    route_ns: &str,
    token: &str,
) -> Option<(usize, std::net::SocketAddr)> {
    for (idx, (name, ns)) in backends.enumerate() {
        let ns = ns.unwrap_or(route_ns);
        let service_key = format!("{}/{}", ns, name);
        let affinity_map = crate::lb::runtime_state::sp_affinity_get_or_build_key(&service_key, || {
            crate::backends::selector::get_backends_for_service_key(&service_key)
        });
        if let Some(&addr) = affinity_map.get(token) {
            return Some((idx, addr));
        }
    }
    None
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
        assert_eq!(t1.len(), 16, "token must be 16 hex chars (64-bit identity)");
        assert!(
            t1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "token must be lowercase hex"
        );
    }

    #[test]
    fn test_token_generation_different_addrs() {
        let t1 = generate_session_token("10.0.0.1:8080");
        let t2 = generate_session_token("10.0.0.2:8080");
        assert_ne!(t1, t2, "different addresses should produce different tokens");
    }

    /// Rule-scoped resolution walks backendRefs in declaration order and finds
    /// the token in a LATER backendRef's Service — the core GEP-1619 fix: a
    /// session bound to Service B must resolve even though weighted selection
    /// might have picked Service A.
    #[test]
    fn test_sp_lookup_rule_scoped_across_services() {
        use crate::lb::runtime_state::{remove_service, sp_affinity_get_or_build_key};
        use crate::lb::EdgionBackend;
        use pingora_load_balancing::Backend;
        use std::net::SocketAddr as StdSocketAddr;
        use std::sync::Arc;

        let make = |addr: StdSocketAddr| {
            Arc::new(EdgionBackend::new(Backend {
                addr: pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
                weight: 1,
                ext: Default::default(),
            }))
        };
        let a_addr: StdSocketAddr = "10.1.0.1:8080".parse().unwrap();
        let b_addr: StdSocketAddr = "10.2.0.1:9090".parse().unwrap();
        // Unique service names: SP_AFFINITY is a process-global cache.
        let svc_a = "rule-scope-test/svc-a";
        let svc_b = "rule-scope-test/svc-b";
        let a_backends = Arc::new(vec![make(a_addr)]);
        let b_backends = Arc::new(vec![make(b_addr)]);
        // Pre-populate the per-service affinity maps (test stand-in for the
        // lazy build off the live backend store).
        sp_affinity_get_or_build_key(svc_a, || a_backends.clone());
        sp_affinity_get_or_build_key(svc_b, || b_backends.clone());

        let refs = [("svc-a", None::<&str>), ("svc-b", None)];
        let token_b = generate_session_token(&b_addr.to_string());
        let hit = sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", &token_b);
        assert_eq!(hit, Some((1, b_addr)), "token bound to svc-b must resolve to index 1");

        let token_a = generate_session_token(&a_addr.to_string());
        let hit = sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", &token_a);
        assert_eq!(hit, Some((0, a_addr)), "token bound to svc-a must resolve to index 0");

        // A token minted for an endpoint outside this rule's backendRefs must
        // miss (rule scoping / stale-endpoint fallback).
        let foreign = generate_session_token("10.9.9.9:7070");
        let miss = sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", &foreign);
        assert_eq!(miss, None, "foreign token must not resolve within this rule");

        remove_service(svc_a);
        remove_service(svc_b);
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
