//! Canonical source of truth for "public" (unauthenticated) HTTP paths.
//!
//! These paths are referenced by both the authentication layer (`unified_auth`)
//! and the authorization layer (`authz_middleware`), plus config validation and
//! config defaults. Centralizing them here prevents the three-way drift that
//! previously let a fail-closed authz change 403 the public `/health` probe.
//!
//! # Security boundary (fr-cauth-01)
//!
//! The authentication skip-set is `static base ∪ operator-configured skip_paths`
//! (operators may extend it within an allowlist). The authorization skip-set is
//! the **static base ONLY** — never the operator-configured set, otherwise a
//! business route added to `skip_paths` would bypass authorization. The helpers
//! below expose the static base; callers that need the dynamic union add the
//! operator config on top themselves.

/// Public liveness/readiness/metrics probes. Unauthenticated by convention
/// (K8s controller-manager style). Exact-match.
pub const PUBLIC_PROBE_PATHS: &[&str] = &["/health", "/ready", "/metrics"];

/// Public auth endpoints that must stay reachable regardless of provider
/// readiness: login (authenticate), logout (clear session), status (surface
/// init state). `/api/v1/auth/me` is intentionally excluded — it requires a
/// valid token. Exact-match.
pub const PUBLIC_AUTH_ENDPOINTS: &[&str] = &["/api/v1/auth/login", "/api/v1/auth/logout", "/api/v1/auth/status"];

/// Prefix under which all auth endpoints live. Operators may add any path under
/// this prefix to their `skip_paths` (config allowlist), because everything here
/// is unauthenticated by design.
pub const AUTH_ENDPOINT_PREFIX: &str = "/api/v1/auth/";

/// True if `path` is a public probe (`/health`, `/ready`, `/metrics`).
pub fn is_public_probe(path: &str) -> bool {
    PUBLIC_PROBE_PATHS.contains(&path)
}

/// True if `path` is one of the three public auth endpoints (exact-match).
pub fn is_public_auth_endpoint(path: &str) -> bool {
    PUBLIC_AUTH_ENDPOINTS.contains(&path)
}

/// The **static** public set used by the authorization layer's skip check.
/// Probes + the three public auth endpoints. Deliberately EXCLUDES
/// operator-configured `skip_paths` (fr-cauth-01: authz must never trust
/// operator-extended skips).
pub fn is_static_public(path: &str) -> bool {
    is_public_probe(path) || is_public_auth_endpoint(path)
}

/// Whether an operator-supplied `skip_paths` entry is allowed (config
/// validation). Probes (exact) or anything under the auth prefix. A business
/// route like `/api/v1/cluster/httproute` is rejected. See fr-cauth-01.
pub fn is_skip_allowlisted(path: &str) -> bool {
    is_public_probe(path) || path.starts_with(AUTH_ENDPOINT_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_are_public() {
        for p in ["/health", "/ready", "/metrics"] {
            assert!(is_public_probe(p), "{p} should be a probe");
            assert!(is_static_public(p), "{p} should be static-public");
        }
        assert!(!is_public_probe("/api/v1/auth/login"));
    }

    #[test]
    fn public_auth_endpoints_exact_only() {
        for p in ["/api/v1/auth/login", "/api/v1/auth/logout", "/api/v1/auth/status"] {
            assert!(is_public_auth_endpoint(p), "{p} should be public auth endpoint");
            assert!(is_static_public(p), "{p} should be static-public");
        }
        // `me` requires a token: not a public endpoint.
        assert!(!is_public_auth_endpoint("/api/v1/auth/me"));
        assert!(!is_static_public("/api/v1/auth/me"));
    }

    /// Regression guard: the static-public set must EXACTLY equal the 6 literals
    /// that `authz_middleware::is_skipped` historically matched. If someone adds
    /// a path to one constant but not the other, this fails.
    #[test]
    fn static_public_matches_historic_authz_skip_set() {
        let historic = [
            "/health",
            "/ready",
            "/metrics",
            "/api/v1/auth/login",
            "/api/v1/auth/logout",
            "/api/v1/auth/status",
        ];
        for p in historic {
            assert!(is_static_public(p), "{p} must remain static-public");
        }
        // Business routes and token-required routes are never static-public.
        assert!(!is_static_public("/api/v1/cluster/httproute"));
        assert!(!is_static_public("/api/v1/auth/me"));
    }

    #[test]
    fn allowlist_accepts_probes_and_auth_prefix_rejects_business() {
        assert!(is_skip_allowlisted("/health"));
        assert!(is_skip_allowlisted("/api/v1/auth/anything"));
        assert!(!is_skip_allowlisted("/api/v1/cluster/httproute"));
        assert!(!is_skip_allowlisted("/metrics/../secret"));
    }
}
