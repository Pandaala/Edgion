//! Session Persistence utilities for HTTP routes.
//!
//! Provides token generation, cookie/header extraction, and Set-Cookie building
//! for Gateway API SessionPersistence support.

use edgion_resources::resources::http_route::{CookieLifetimeType, SessionPersistence, SessionPersistenceType};
use pingora_proxy::Session;
use sha2::{Digest, Sha256};

use crate::{EdgionHttpContext, SessionPersistenceState};

pub(crate) struct StickyCandidate {
    pub slot: usize,
    pub service_key: String,
    pub scope: Option<String>,
}

/// Current Unix time in whole seconds (call edge; pure logic takes `now`).
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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
    generate_scoped_session_token(None, backend_addr)
}

/// Generate the canonical token with an optional typed routing scope.
/// Ordinary Service callers pass `None` and retain the existing wire format.
pub fn generate_scoped_session_token(scope: Option<&str>, backend_addr: &str) -> String {
    let mut hasher = Sha256::new();
    if let Some(scope) = scope {
        hasher.update(scope.as_bytes());
        hasher.update([0]);
    }
    hasher.update(backend_addr.as_bytes());
    let digest = hasher.finalize();
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
    now: u64,
) -> Option<String> {
    let sp = sp?;
    let raw = extract_session_token(session, sp);
    sp_apply_extracted_token(raw, sp_state, now)
}

/// Post-extraction logic shared by `sp_extract_token` and unit tests: given the
/// raw extracted wire token (if any) and `now`, set the SP state and return the
/// bare `addrhash` to look up, or `None` when absent/expired.
///
/// - Absent → `InjectNew`, `None`.
/// - Present, expired (`now >= expiry`) → `InjectNew`, `None` (behave like a
///   missing token so the caller falls back to weighted selection and refreshes).
/// - Present, live → `Inactive`, `Some(addrhash)` (expiry stripped; the affinity
///   map is keyed by the bare `addrhash`).
fn sp_apply_extracted_token(raw: Option<String>, sp_state: &mut SessionPersistenceState, now: u64) -> Option<String> {
    match raw {
        None => {
            *sp_state = SessionPersistenceState::InjectNew;
            None
        }
        Some(token) => {
            let (hash, expiry) = parse_wire_token(&token);
            if expiry.is_some_and(|e| now >= e) {
                *sp_state = SessionPersistenceState::InjectNew;
                None
            } else {
                *sp_state = SessionPersistenceState::Inactive;
                Some(hash.to_string())
            }
        }
    }
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
    sp_resolve_target_rule_scoped_indexed(
        ctx,
        backends
            .enumerate()
            .map(|(index, (name, namespace))| (index, name, namespace)),
        route_ns,
        token,
    )
}

/// Indexed form used when a typed backend list filters non-Service slots but
/// must still return the original `backendRefs` position.
pub(crate) fn sp_resolve_target_rule_scoped_indexed<'a>(
    ctx: &mut EdgionHttpContext,
    backends: impl Iterator<Item = (usize, &'a str, Option<&'a str>)>,
    route_ns: &str,
    token: Option<&str>,
) -> Option<usize> {
    let token = token?;
    match sp_lookup_rule_scoped_indexed(backends, route_ns, token) {
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
#[cfg(test)]
fn sp_lookup_rule_scoped<'a>(
    backends: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    route_ns: &str,
    token: &str,
) -> Option<(usize, std::net::SocketAddr)> {
    sp_lookup_rule_scoped_indexed(
        backends
            .enumerate()
            .map(|(index, (name, namespace))| (index, name, namespace)),
        route_ns,
        token,
    )
}

fn sp_lookup_rule_scoped_indexed<'a>(
    backends: impl Iterator<Item = (usize, &'a str, Option<&'a str>)>,
    route_ns: &str,
    token: &str,
) -> Option<(usize, std::net::SocketAddr)> {
    for (idx, name, ns) in backends {
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

pub(crate) fn sp_resolve_typed_candidates(
    ctx: &mut EdgionHttpContext,
    candidates: &[StickyCandidate],
    token: Option<&str>,
) -> Option<usize> {
    let token = token?;
    for candidate in candidates {
        let affinity_map = crate::lb::runtime_state::sp_affinity_get_or_build_scoped(
            &candidate.service_key,
            candidate.scope.as_deref(),
            || crate::backends::selector::get_backends_for_service_key(&candidate.service_key),
        );
        if let Some(&addr) = affinity_map.get(token) {
            ctx.sp_target_addr = Some(addr);
            ctx.sp_sticky_addr = Some(addr);
            ctx.sp_token_scope = candidate.scope.clone();
            return Some(candidate.slot);
        }
    }
    ctx.sp_state = SessionPersistenceState::InjectNew;
    None
}

/// Build the wire token. `<addrhash>` alone when no lifetime is configured;
/// `<addrhash>.<now+ttl_secs>` (absolute expiry epoch, frozen at issuance) when
/// it is. `<addrhash>` remains the affinity-map key.
pub fn build_wire_token(addrhash: &str, now: u64, ttl_secs: Option<u64>) -> String {
    match ttl_secs {
        Some(ttl) => format!("{addrhash}.{}", now.saturating_add(ttl)),
        None => addrhash.to_string(),
    }
}

/// Split a wire token into `(addrhash, Option<expiry_epoch>)`. An unparseable
/// expiry component is treated as "no expiry" so a malformed token never
/// mis-expires (it will simply miss the affinity map and refresh).
fn parse_wire_token(token: &str) -> (&str, Option<u64>) {
    match token.split_once('.') {
        Some((hash, exp)) => (hash, exp.parse::<u64>().ok()),
        None => (token, None),
    }
}

/// Build a Set-Cookie header value for session persistence.
///
/// Format: `name=value; Path=/[; Max-Age=N]; HttpOnly; SameSite=Lax`
/// `token` is the full wire token (already carrying any expiry). `Max-Age` is
/// emitted only for a Permanent lifetime and uses the same `ttl_secs` the wire
/// token's expiry was derived from (single source of truth — no re-parse).
pub fn build_set_cookie_value(sp: &SessionPersistence, token: &str, is_tls: bool, ttl_secs: Option<u64>) -> String {
    let name = get_session_name(sp);
    let mut cookie = format!("{}={}; Path=/", name, token);

    let is_permanent = sp
        .cookie_config
        .as_ref()
        .and_then(|c| c.lifetime_type.as_ref())
        .map(|lt| *lt == CookieLifetimeType::Permanent)
        .unwrap_or(false);

    if is_permanent {
        if let Some(secs) = ttl_secs {
            cookie.push_str(&format!("; Max-Age={secs}"));
        }
    }

    cookie.push_str("; HttpOnly; SameSite=Lax");
    if is_tls {
        cookie.push_str("; Secure");
    }
    cookie
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

    #[test]
    fn scoped_token_binds_direct_and_discovery_identity() {
        let addr = "10.0.0.1:8080";
        let first =
            generate_scoped_session_token(Some("edgion.io/EdgionBackend/ns/first\0core/Service/ns/shared"), addr);
        let other_direct =
            generate_scoped_session_token(Some("edgion.io/EdgionBackend/ns/second\0core/Service/ns/shared"), addr);
        let other_discovery =
            generate_scoped_session_token(Some("edgion.io/EdgionBackend/ns/first\0core/Service/ns/other"), addr);

        assert_ne!(first, other_direct);
        assert_ne!(first, other_discovery);
        assert_eq!(generate_scoped_session_token(None, addr), generate_session_token(addr));
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
        let indexed_refs = [(0, "svc-a", None::<&str>), (2, "svc-b", None)];
        let indexed_hit = sp_lookup_rule_scoped_indexed(indexed_refs.into_iter(), "rule-scope-test", &token_b);
        assert_eq!(
            indexed_hit,
            Some((2, b_addr)),
            "typed filtering must retain the raw backendRef slot"
        );

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
    fn test_build_wire_token_optional_expiry() {
        // No ttl -> bare addrhash (unchanged behavior).
        assert_eq!(build_wire_token("1a2b3c4d5e6f7a8b", 1000, None), "1a2b3c4d5e6f7a8b");
        // ttl -> addrhash.<now+ttl>
        assert_eq!(
            build_wire_token("1a2b3c4d5e6f7a8b", 1000, Some(60)),
            "1a2b3c4d5e6f7a8b.1060"
        );
    }

    #[test]
    fn test_parse_wire_token() {
        assert_eq!(parse_wire_token("abc123"), ("abc123", None));
        assert_eq!(parse_wire_token("abc123.1060"), ("abc123", Some(1060)));
        // Unparseable expiry component -> treated as no expiry (never mis-expires).
        assert_eq!(parse_wire_token("abc123.xyz"), ("abc123", None));
    }

    #[test]
    fn test_extract_token_expiry() {
        // Past expiry: now (1000) >= expiry (500) -> expired -> InjectNew, None.
        let mut st = SessionPersistenceState::Inactive;
        let got = sp_apply_extracted_token(Some("aaaabbbbccccdddd.500".to_string()), &mut st, 1000);
        assert_eq!(got, None);
        assert!(matches!(st, SessionPersistenceState::InjectNew));

        // Future expiry: live -> Inactive, returns bare addrhash (expiry stripped).
        let mut st2 = SessionPersistenceState::Inactive;
        let got2 = sp_apply_extracted_token(Some("aaaabbbbccccdddd.5000".to_string()), &mut st2, 1000);
        assert_eq!(got2.as_deref(), Some("aaaabbbbccccdddd"));
        assert!(matches!(st2, SessionPersistenceState::Inactive));

        // No expiry component: live forever -> Inactive, bare hash.
        let mut st3 = SessionPersistenceState::Inactive;
        let got3 = sp_apply_extracted_token(Some("aaaabbbbccccdddd".to_string()), &mut st3, 1000);
        assert_eq!(got3.as_deref(), Some("aaaabbbbccccdddd"));
        assert!(matches!(st3, SessionPersistenceState::Inactive));

        // Absent token -> InjectNew, None.
        let mut st4 = SessionPersistenceState::Inactive;
        let got4 = sp_apply_extracted_token(None, &mut st4, 1000);
        assert_eq!(got4, None);
        assert!(matches!(st4, SessionPersistenceState::InjectNew));
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
        // Session lifetime: no Max-Age even though a ttl is supplied.
        let cookie = build_set_cookie_value(&sp, "abcd1234", false, Some(3600));
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
        let cookie = build_set_cookie_value(&sp, "abcd1234", false, Some(86400));
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
        let cookie = build_set_cookie_value(&sp, "token123", false, Some(3600));
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
        let cookie = build_set_cookie_value(&sp, "abcd1234", true, None);
        assert_eq!(
            cookie,
            "edgion-session-affinity=abcd1234; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
    }
}
