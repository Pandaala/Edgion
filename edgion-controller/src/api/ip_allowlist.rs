//! Admin API IP allowlist (L7 enforcement, fr admin-api-02).
//!
//! Rejects requests whose TCP peer IP is not in the operator-configured CIDR
//! allowlist, BEFORE authentication. The decision input is the kernel TCP peer
//! (`ConnectInfo<SocketAddr>`) — never X-Forwarded-For, which is spoofable.
//!
//! - Empty/unset allowlist ⇒ the layer is not mounted (allow all; see callers).
//! - Missing `ConnectInfo` ⇒ 403 (fail-closed): guards a serve entry that forgot
//!   `into_make_service_with_connect_info`.
//! - IPv4-mapped IPv6 peers (`::ffff:a.b.c.d` on a `::`-bound listener) are
//!   unmapped via `IpAddr::to_canonical()` before matching.
//!
//! Reuses the existing `ip_cidr_helpers` / `ip_radix_tree` matcher — no new crate.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::matcher::ip_cidr_helpers::{to_cidr_format, validate_ip_or_cidr};

/// Single group label for the flat admin allowlist matcher.
const ADMIN_ALLOW_GROUP: &str = "admin-allow";

/// Validate every CIDR entry (fail-fast at config load). Reuses
/// `validate_ip_or_cidr`; does not build a matcher.
pub fn validate_admin_ips(cidrs: &[String]) -> anyhow::Result<()> {
    for cidr in cidrs {
        validate_ip_or_cidr(cidr).map_err(|e| anyhow::anyhow!("invalid allow_admin_ips entry: {}", e))?;
    }
    Ok(())
}

/// Build the allowlist matcher. `Ok(None)` when empty (allow-all: caller must not
/// mount the layer). `Err` on a malformed CIDR (defense in depth; load validates).
pub fn build_admin_ip_matcher(cidrs: &[String]) -> anyhow::Result<Option<Arc<IpRadixMatcher>>> {
    if cidrs.is_empty() {
        return Ok(None);
    }
    let mut builder = IpRadixMatcher::builder();
    for cidr in cidrs {
        let norm =
            to_cidr_format(cidr).map_err(|e| anyhow::anyhow!("invalid allow_admin_ips entry '{}': {}", cidr, e))?;
        builder
            .insert(&norm, ADMIN_ALLOW_GROUP)
            .map_err(|e| anyhow::anyhow!("invalid allow_admin_ips entry '{}': {}", cidr, e))?;
    }
    let matcher = builder
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build admin IP allowlist matcher: {}", e))?;
    Ok(Some(Arc::new(matcher)))
}

/// Exact 403 body shape (field order guaranteed by struct declaration order).
#[derive(serde::Serialize)]
struct DeniedBody {
    success: bool,
    error: &'static str,
}

fn denied_response() -> Response {
    let mut resp = (
        StatusCode::FORBIDDEN,
        axum::Json(DeniedBody {
            success: false,
            error: "Access denied",
        }),
    )
        .into_response();
    // The IP layer sits OUTSIDE compose's cache-control middleware, so set it here
    // to keep the `Cache-Control: no-store` contract on every admin response.
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Rate-limited denial log: 1st, then every 256th, to bound volume under a flood.
fn log_denied(peer: Option<std::net::IpAddr>) {
    static DENIED: AtomicU64 = AtomicU64::new(0);
    let n = DENIED.fetch_add(1, Ordering::Relaxed);
    if n == 0 || n.is_multiple_of(256) {
        tracing::warn!(
            component = "admin_ip_allowlist",
            peer = ?peer,
            denied_total = n + 1,
            "admin API request denied by IP allowlist"
        );
    }
}

/// Middleware: pass through when the (canonicalized) TCP peer matches the
/// allowlist; otherwise 403. Fail-closed when `ConnectInfo` is absent.
pub async fn ip_allowlist_middleware(
    State(matcher): State<Arc<IpRadixMatcher>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_canonical());
    match peer {
        Some(ip) if matcher.matched_group(&ip).is_some() => next.run(req).await,
        other => {
            log_denied(other);
            denied_response()
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/api/ip_allowlist_test.rs"]
mod tests;
