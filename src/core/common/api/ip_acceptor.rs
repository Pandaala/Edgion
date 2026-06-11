//! Admin API L4 TCP IP pre-filter (fr admin-api-04).
//!
//! Drops TCP connections whose kernel peer IP is not in the operator-configured
//! `allow_tcp_ips` CIDR allowlist, BEFORE the rustls handshake, on Center's HTTPS
//! admin listener. Implemented as a custom `axum_server::accept::Accept` placed
//! before `RustlsAcceptor`: returning `Err` aborts the connection pre-handshake.
//!
//! Independent from the L7 `ip_allowlist` middleware — separate config field,
//! module, and denial counter. Only the lower-level `edgion_resources::matcher`
//! library is shared.
//!
//! Precondition: the decision input is `TcpStream::peer_addr()` (the kernel TCP
//! peer). The feature is only correct when that is the real client — behind an
//! SNAT load balancer / terminating proxy it either no-ops or drops everything.
//! See the design spec for the full topology requirements.

use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use metrics::counter;
use tokio::net::TcpStream;

use edgion_resources::matcher::ip_cidr_helpers::{to_cidr_format, validate_ip_or_cidr};
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;

/// Single group label for the flat L4 allowlist matcher.
const TCP_ALLOW_GROUP: &str = "tcp-allow";

/// Metric name kept as a const to match the codebase's name-as-const discipline
/// (every `counter!` call site uses a const, never a string literal). Prefix is
/// `edgion_l4_` (an admin-listener L4 metric), not `edgion_fed_`.
const DENIED_TOTAL: &str = "edgion_l4_tcp_denied_total";

/// Validate every CIDR entry (fail-fast at config load). Reuses
/// `validate_ip_or_cidr`; does not build a matcher.
pub fn validate_tcp_ips(cidrs: &[String]) -> anyhow::Result<()> {
    for cidr in cidrs {
        validate_ip_or_cidr(cidr).map_err(|e| anyhow::anyhow!("invalid allow_tcp_ips entry: {}", e))?;
    }
    Ok(())
}

/// Build the L4 allowlist matcher. `Ok(None)` when empty (allow-all: caller must
/// not mount the acceptor). `Err` on a malformed CIDR (defense in depth).
pub fn build_tcp_ip_matcher(cidrs: &[String]) -> anyhow::Result<Option<Arc<IpRadixMatcher>>> {
    if cidrs.is_empty() {
        return Ok(None);
    }
    let mut builder = IpRadixMatcher::builder();
    for cidr in cidrs {
        let norm =
            to_cidr_format(cidr).map_err(|e| anyhow::anyhow!("invalid allow_tcp_ips entry '{}': {}", cidr, e))?;
        builder
            .insert(&norm, TCP_ALLOW_GROUP)
            .map_err(|e| anyhow::anyhow!("invalid allow_tcp_ips entry '{}': {}", cidr, e))?;
    }
    let matcher = builder
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build allow_tcp_ips matcher: {}", e))?;
    Ok(Some(Arc::new(matcher)))
}

/// Pure decision predicate: the (canonicalized) peer IP matches the allowlist.
/// IPv4-mapped IPv6 (`::ffff:a.b.c.d`) is unmapped via `to_canonical()` first,
/// matching the L7 middleware's behavior.
fn tcp_ip_allowed(matcher: &IpRadixMatcher, ip: IpAddr) -> bool {
    matcher.matched_group(&ip.to_canonical()).is_some()
}

/// Custom acceptor placed BEFORE `RustlsAcceptor`. Runs the IP check on the raw
/// `TcpStream`; on a non-allowlisted peer it returns `Err`, which `axum-server`
/// turns into a silent connection drop *before* the TLS handshake.
///
/// Must be `Clone + Send + Sync + 'static` — `axum-server`'s `serve` requires it,
/// and `RustlsAcceptor`'s `Clone` is conditional on the inner acceptor's.
#[derive(Clone)]
pub struct TcpIpAcceptor {
    matcher: Arc<IpRadixMatcher>,
}

impl TcpIpAcceptor {
    pub fn new(matcher: Arc<IpRadixMatcher>) -> Self {
        Self { matcher }
    }
}

impl<S> axum_server::accept::Accept<TcpStream, S> for TcpIpAcceptor {
    type Stream = TcpStream;
    type Service = S;
    type Future = std::future::Ready<io::Result<(TcpStream, S)>>;

    // Synchronous (returns a `Ready` future), mirroring axum-server's own
    // `NoDelayAcceptor`. Keep it non-blocking: `RustlsAcceptor::handshake_timeout`
    // wraps only the TLS future, NOT this inner acceptor, so any async work added
    // here would be unprotected by a timeout.
    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        match stream.peer_addr() {
            Ok(addr) if tcp_ip_allowed(&self.matcher, addr.ip()) => {
                record_first_admitted(addr.ip());
                std::future::ready(Ok((stream, service)))
            }
            other => {
                record_denied(other.ok().map(|a| a.ip()));
                std::future::ready(Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "tcp ip not allowed",
                )))
            }
        }
    }
}

/// Log the peer of the FIRST admitted connection exactly once (cheap atomic load
/// thereafter — no per-connection data-plane logging). Lets an operator verify the
/// kernel TCP peer is the real client and not an SNAT/LB address: behind an SNAT
/// load balancer this line shows the LB IP, exposing an otherwise-silent
/// "allow-all in disguise" misconfiguration.
fn record_first_admitted(peer: IpAddr) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if LOGGED
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        tracing::info!(
            component = "tcp_ip_acceptor",
            peer = %peer,
            "L4 TCP IP pre-filter admitted first connection; verify this peer is the real \
             client IP and not an SNAT/load-balancer address"
        );
    }
}

/// Record a denied connection: always bump the Prometheus counter; rate-limit the
/// WARN log (1st, then every 256th) to bound volume under a flood. Independent
/// from the L7 allowlist's counter/log.
fn record_denied(peer: Option<IpAddr>) {
    counter!(DENIED_TOTAL).increment(1);
    static DENIED: AtomicU64 = AtomicU64::new(0);
    let n = DENIED.fetch_add(1, Ordering::Relaxed);
    if n == 0 || n.is_multiple_of(256) {
        tracing::warn!(
            component = "tcp_ip_acceptor",
            peer = ?peer,
            denied_total = n + 1,
            "L4 TCP connection denied by allowlist"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_server::accept::Accept;
    use tokio::net::TcpListener;

    fn matcher(cidrs: &[&str]) -> Arc<IpRadixMatcher> {
        let v: Vec<String> = cidrs.iter().map(|s| s.to_string()).collect();
        build_tcp_ip_matcher(&v).unwrap().unwrap()
    }

    #[test]
    fn ipv4_in_cidr_allowed() {
        let m = matcher(&["10.0.0.0/8"]);
        assert!(tcp_ip_allowed(&m, "10.1.2.3".parse().unwrap()));
    }

    #[test]
    fn ipv4_outside_cidr_denied() {
        let m = matcher(&["10.0.0.0/8"]);
        assert!(!tcp_ip_allowed(&m, "192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn ipv6_in_cidr_allowed() {
        let m = matcher(&["2001:db8::/32"]);
        assert!(tcp_ip_allowed(&m, "2001:db8::1".parse().unwrap()));
    }

    /// Key regression: a dual-stack listener reports an IPv4 client as
    /// `::ffff:10.1.2.3`; it must match an IPv4 CIDR after to_canonical().
    #[test]
    fn ipv4_mapped_ipv6_matches_ipv4_cidr() {
        let m = matcher(&["10.0.0.0/8"]);
        assert!(tcp_ip_allowed(&m, "::ffff:10.1.2.3".parse().unwrap()));
    }

    #[test]
    fn empty_allowlist_returns_none() {
        assert!(build_tcp_ip_matcher(&[]).unwrap().is_none());
    }

    #[test]
    fn malformed_cidr_errors() {
        assert!(build_tcp_ip_matcher(&["10.0.0/8".to_string()]).is_err());
        assert!(validate_tcp_ips(&["10.0.0/8".to_string()]).is_err());
        assert!(validate_tcp_ips(&["10.0.0.0/8".to_string(), "::1".to_string()]).is_ok());
    }

    #[test]
    fn metric_name_is_stable() {
        assert_eq!(DENIED_TOTAL, "edgion_l4_tcp_denied_total");
    }

    /// Accept a real loopback TCP connection and run the actual `Accept` impl.
    /// Returns whether the acceptor admitted the connection (Ok) or dropped it (Err).
    async fn accept_decision(allow: &[&str]) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move { TcpStream::connect(addr).await.unwrap() });
        let (server_stream, _) = listener.accept().await.unwrap();
        let _client = client.await.unwrap();

        let acceptor = TcpIpAcceptor::new(matcher(allow));
        // Service type is irrelevant to the IP decision; use ().
        acceptor.accept(server_stream, ()).await.is_ok()
    }

    /// Allowlisted loopback peer is admitted (would proceed to TLS).
    #[tokio::test]
    async fn real_socket_allowed_peer_admitted() {
        assert!(accept_decision(&["127.0.0.0/8"]).await);
    }

    /// Non-allowlisted peer is dropped: the acceptor returns Err, which is exactly
    /// the signal axum-server uses to abort the connection before the TLS handshake.
    #[tokio::test]
    async fn real_socket_denied_peer_dropped() {
        assert!(!accept_decision(&["10.0.0.0/8"]).await);
    }
}
