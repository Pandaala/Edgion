//! Session Persistence utilities for HTTP routes.
//!
//! Provides token generation, cookie/header extraction, and Set-Cookie building
//! for Gateway API SessionPersistence support.

use std::fmt::Write as _;

use edgion_resources::resources::http_route::{CookieLifetimeType, SessionPersistence, SessionPersistenceType};
use pingora_proxy::Session;
use sha2::{Digest, Sha256};

use crate::plugins::runtime::session_adapter::cookie_value_ref;
use crate::{EdgionHttpContext, SessionPersistenceState};

pub(crate) struct StickyCandidate<'a> {
    pub slot: usize,
    pub service_key: &'a str,
    pub section: &'a crate::backends::discovery::ServiceSectionKey,
    pub scope: Option<&'a str>,
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

/// Extract a session token from all Cookie fields using canonical first-match semantics.
pub fn extract_token_from_cookie<'a>(session: &'a Session, cookie_name: &str) -> Option<&'a str> {
    cookie_value_ref(&session.req_header().headers, cookie_name)
}

/// Extract a session token from a custom header.
pub fn extract_token_from_header<'a>(session: &'a Session, header_name: &str) -> Option<&'a str> {
    let req_header = session.req_header();
    req_header.headers.get(header_name).and_then(|v| v.to_str().ok())
}

/// Unified extraction: uses cookie or header based on the persistence type config.
pub fn extract_session_token<'a>(session: &'a Session, sp: &SessionPersistence) -> Option<&'a str> {
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
pub(crate) fn sp_extract_token<'a>(
    session: &'a Session,
    sp_state: &mut SessionPersistenceState,
    sp: Option<&SessionPersistence>,
    now: u64,
) -> Option<&'a str> {
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
/// - Malformed expiry → retain the whole token so affinity lookup misses and refreshes.
fn sp_apply_extracted_token<'a>(
    raw: Option<&'a str>,
    sp_state: &mut SessionPersistenceState,
    now: u64,
) -> Option<&'a str> {
    match raw {
        None => {
            *sp_state = SessionPersistenceState::InjectNew;
            None
        }
        Some(token) => {
            let (hash, expiry) = parse_wire_token(token);
            if expiry.is_some_and(|e| now >= e) {
                *sp_state = SessionPersistenceState::InjectNew;
                None
            } else {
                *sp_state = SessionPersistenceState::Inactive;
                Some(hash)
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
/// - Token hits some backendRef's Service affinity map → set `target_addr`
///   so `try_get_peer` routes directly to it; return that backendRef's index
///   so the caller selects the SAME backendRef (TLS policy + backend plugins
///   must follow the sticky Service, not a fresh WRR pick).
/// - Token misses every backendRef (endpoint or backendRef gone) → flip state
///   to `InjectNew` so the caller falls back to weighted selection and the
///   response refreshes the token.
pub(crate) fn sp_resolve_typed_candidates<'a>(
    ctx: &mut EdgionHttpContext,
    candidates: impl Iterator<Item = StickyCandidate<'a>>,
    token: Option<&str>,
) -> Option<usize> {
    let token = token?;
    let found = sp_lookup_typed_candidates(candidates, token, |candidate, token| {
        let affinity_map = crate::lb::runtime_state::sp_affinity_get_or_build_scoped(
            candidate.service_key,
            candidate.section,
            candidate.scope,
            || crate::backends::selector::backends_for_service_section(candidate.service_key, candidate.section),
        );
        affinity_map.get(token).copied()
    });
    if let Some((slot, addr, scope)) = found {
        let persistence = ctx.ensure_session_persistence();
        persistence.target_addr = Some(addr);
        persistence.sticky_addr = Some(addr);
        persistence.token_scope = scope.map(str::to_owned);
        Some(slot)
    } else {
        ctx.ensure_session_persistence().state = SessionPersistenceState::InjectNew;
        None
    }
}

fn sp_lookup_typed_candidates<'a>(
    candidates: impl Iterator<Item = StickyCandidate<'a>>,
    token: &str,
    mut lookup: impl FnMut(&StickyCandidate<'a>, &str) -> Option<std::net::SocketAddr>,
) -> Option<(usize, std::net::SocketAddr, Option<&'a str>)> {
    for candidate in candidates {
        if let Some(addr) = lookup(&candidate, token) {
            return Some((candidate.slot, addr, candidate.scope));
        }
    }
    None
}

#[cfg(test)]
include!("../../../tests/support/routes/http/session_persistence_test/230_function_item.rs");

/// Build the wire token. `<addrhash>` alone when no lifetime is configured;
/// `<addrhash>.<now+ttl_secs>` (absolute expiry epoch, frozen at issuance) when
/// it is. `<addrhash>` remains the affinity-map key.
pub fn build_wire_token(addrhash: String, now: u64, ttl_secs: Option<u64>) -> String {
    match ttl_secs {
        Some(ttl) => {
            let expiry = now.saturating_add(ttl);
            let digits = expiry.checked_ilog10().unwrap_or(0) as usize + 1;
            let mut token = String::with_capacity(addrhash.len() + 1 + digits);
            let _ = write!(token, "{addrhash}.{expiry}");
            token
        }
        None => addrhash,
    }
}

/// Split a wire token into `(addrhash, Option<expiry_epoch>)`. An unparseable
/// expiry component retains the whole token as the lookup key, so it misses
/// the affinity map instead of adopting a valid hash with its expiry removed.
fn parse_wire_token(token: &str) -> (&str, Option<u64>) {
    match token.split_once('.') {
        Some((hash, exp)) => match exp.parse::<u64>() {
            Ok(expiry) => (hash, Some(expiry)),
            Err(_) => (token, None),
        },
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
    let is_permanent = sp
        .cookie_config
        .as_ref()
        .and_then(|c| c.lifetime_type.as_ref())
        .map(|lt| *lt == CookieLifetimeType::Permanent)
        .unwrap_or(false);

    let max_age = if is_permanent { ttl_secs } else { None };
    let max_age_len = max_age.map_or(0, |secs| {
        "; Max-Age=".len() + secs.checked_ilog10().unwrap_or(0) as usize + 1
    });
    let suffix = "; HttpOnly; SameSite=Lax";
    let secure = if is_tls { "; Secure" } else { "" };
    let capacity = name.len() + 1 + token.len() + "; Path=/".len() + max_age_len + suffix.len() + secure.len();
    let mut cookie = String::with_capacity(capacity);
    let _ = write!(cookie, "{name}={token}; Path=/");
    if let Some(secs) = max_age {
        let _ = write!(cookie, "; Max-Age={secs}");
    }
    cookie.push_str(suffix);
    cookie.push_str(secure);
    cookie
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/http/session_persistence_test.rs"]
mod tests;
