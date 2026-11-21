#[cfg(all(test, feature = "boringssl"))]
use super::prepared_backend_tls::build_client_cert_key_from_secret;
#[cfg(any(feature = "boringssl", test))]
use crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store;
use crate::backends::policy::backend_tls::backend_tls_policy_store::BACKEND_TLS_PROFILE_DOMAIN;
use crate::backends::policy::{BackendTlsPolicyLookup, BackendTlsProfileIdentity, EffectiveBackendTlsPolicy};
use crate::util::net::{classify_backend_addr, BlockedBackendReason};
use crate::EdgionHttpContext;
#[cfg(all(test, feature = "boringssl"))]
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
#[cfg(test)]
use edgion_resources::resources::BackendTLSPolicy;
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
#[cfg(all(test, feature = "boringssl"))]
use pingora_core::tls::x509::X509;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::Arc;

// Focused pool-isolation tests exercise the final Pingora reuse hash.
#[cfg(all(test, feature = "boringssl"))]
#[path = "../../tests/unit/backends/peer_builder_test/pool_tests.rs"]
mod pool_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendAppProtocol {
    H2c,
    WebSocket,
}

pub(crate) fn parse_backend_app_protocol(value: &str) -> Option<BackendAppProtocol> {
    if value.eq_ignore_ascii_case("kubernetes.io/h2c") || value.eq_ignore_ascii_case("h2c") {
        Some(BackendAppProtocol::H2c)
    } else if value.eq_ignore_ascii_case("kubernetes.io/ws")
        || value.eq_ignore_ascii_case("ws")
        || value.eq_ignore_ascii_case("kubernetes.io/wss")
        || value.eq_ignore_ascii_case("wss")
    {
        Some(BackendAppProtocol::WebSocket)
    } else {
        None
    }
}

pub(crate) fn apply_backend_app_protocol(
    peer: &mut Box<HttpPeer>,
    app_protocol: Option<BackendAppProtocol>,
    ctx: &mut EdgionHttpContext,
) {
    match app_protocol {
        Some(BackendAppProtocol::H2c) => {
            peer.options.set_http_version(2, 2);
        }
        Some(BackendAppProtocol::WebSocket) => {
            if let Some(plan) = &ctx.grpc_transcoding {
                plan.reject_mutation();
                return;
            }
            peer.options.set_http_version(1, 1);
            if ctx.request_info.discover_protocol.is_none() {
                ctx.request_info.discover_protocol = Some("websocket");
            }
        }
        None => {}
    }
}

/// Extract TLS configuration from a usable BackendTLSPolicy lookup.
///
/// `Invalid` is rejected before this helper is called.
#[inline]
pub(crate) fn extract_tls_config(lookup: &BackendTlsPolicyLookup) -> (bool, &str) {
    match lookup {
        BackendTlsPolicyLookup::Effective(effective) => (true, effective.policy.spec.validation.hostname.as_str()),
        BackendTlsPolicyLookup::Absent => (false, ""),
        BackendTlsPolicyLookup::Invalid => (false, ""),
    }
}

#[cfg(any(feature = "boringssl", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientIdentitySource {
    Policy,
    Gateway,
}

#[cfg(any(feature = "boringssl", test))]
#[inline]
fn select_client_identity_source(
    policy_present: bool,
    policy_identity_present: bool,
    gateway_identity_present: bool,
) -> Option<ClientIdentitySource> {
    if !policy_present {
        None
    } else if policy_identity_present {
        Some(ClientIdentitySource::Policy)
    } else if gateway_identity_present {
        Some(ClientIdentitySource::Gateway)
    } else {
        None
    }
}

pub(crate) fn peer_from_backend_tls_lookup(
    addr: SocketAddr,
    lookup: &BackendTlsPolicyLookup,
    gateway_key: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
    if !crate::backends::policy::ancestor_admitted(lookup, None, gateway_key) {
        return Err(EdgionErrorCode::BackendTlsPolicyInvalid);
    }
    match lookup {
        BackendTlsPolicyLookup::Absent => Ok(Box::new(HttpPeer::new(addr, false, String::new()))),
        BackendTlsPolicyLookup::Effective(effective) => create_tls_peer(addr, effective, gateway_key),
        BackendTlsPolicyLookup::Invalid => Err(EdgionErrorCode::BackendTlsPolicyInvalid),
    }
}

pub(crate) fn create_tls_peer(
    addr: SocketAddr,
    effective: &EffectiveBackendTlsPolicy,
    gateway_key: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
    let policy = &effective.policy;
    let sni = policy.spec.validation.hostname.as_str();

    #[cfg(feature = "boringssl")]
    let policy_identity_present = policy.spec.resolved_client_certificate.is_some();

    #[cfg(feature = "boringssl")]
    let gateway_identity = if policy_identity_present {
        None
    } else {
        gateway_key.and_then(|key| get_global_gateway_backend_tls_store().get_prepared(key))
    };

    #[cfg(feature = "boringssl")]
    let mut peer = match select_client_identity_source(true, policy_identity_present, gateway_identity.is_some()) {
        Some(ClientIdentitySource::Policy) => {
            let cert_key = effective
                .prepared
                .identity
                .as_ref()
                .ok_or(EdgionErrorCode::Unknown)?
                .clone()?;
            HttpPeer::new_mtls(addr, sni.to_owned(), cert_key)
        }
        Some(ClientIdentitySource::Gateway) => {
            let cert_key = gateway_identity
                .as_ref()
                .ok_or(EdgionErrorCode::Unknown)?
                .prepared
                .identity
                .clone()?;
            HttpPeer::new_mtls(addr, sni.to_owned(), cert_key)
        }
        None => HttpPeer::new(addr, true, sni.to_owned()),
    };

    #[cfg(not(feature = "boringssl"))]
    let mut peer = {
        let _ = gateway_key;
        HttpPeer::new(addr, true, sni.to_owned())
    };

    #[cfg(feature = "boringssl")]
    if !policy.spec.use_system_ca {
        // Operator opted out of the system trust store, so a missing/empty CA
        // bundle must fail-closed rather than silently fall back to it.
        match effective.prepared.ca.clone()? {
            Some(ca_chain) => peer.options.ca = Some(ca_chain),
            None => return Err(EdgionErrorCode::BackendTlsCaInvalid),
        }
    }

    #[cfg(feature = "boringssl")]
    {
        peer.group_key = backend_tls_group_key(
            &effective.profile,
            gateway_identity.as_ref().map(|entry| &entry.prepared.fingerprint),
        );
        peer.options.upstream_tls_handshake_complete_hook = effective.prepared.san_hook.clone();
    }

    #[cfg(not(feature = "boringssl"))]
    {
        peer.group_key = effective.profile.group_key();
    }

    Ok(Box::new(peer))
}

fn backend_tls_group_key(profile: &BackendTlsProfileIdentity, gateway_fingerprint: Option<&[u8; 32]>) -> u64 {
    let Some(secret_fp) = gateway_fingerprint else {
        return profile.group_key();
    };
    let mut digest = Sha256::new();
    digest.update(BACKEND_TLS_PROFILE_DOMAIN);
    digest.update(profile.fingerprint);
    digest.update((secret_fp.len() as u64).to_be_bytes());
    digest.update(secret_fp);
    let out: [u8; 32] = digest.finalize().into();
    u64::from_be_bytes(out[..8].try_into().expect("sha256 is 32 bytes"))
}

/// Record TLS configuration to upstream context.
///
/// Writes `tls` unconditionally (Some when TLS is in use, None otherwise) so
/// that retries / SP fallbacks reusing the same placeholder upstream entry
/// cannot leave stale TLS metadata from an earlier attempt.
#[inline]
pub(crate) fn record_tls_to_upstream(ctx: &mut EdgionHttpContext, use_tls: bool, sni: &str) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.tls = if use_tls {
            Some(crate::BackendTlsInfo {
                sni: if sni.is_empty() { None } else { Some(sni.to_string()) },
                handshake_ok: None,
                protocol: None,
                cipher: None,
            })
        } else {
            None
        };
    }
}

/// Validate backend address for security.
///
/// Rejects:
/// - Loopback (`127.0.0.0/8`, `::1`) — typical SSRF-to-host
/// - Link-local (`169.254.0.0/16`, `fe80::/10`) — covers cloud metadata (`169.254.169.254`)
/// - Operator-configured `outbound_egress_blocked_cidrs`
/// - Non-dialable destinations, including invalid transition endpoints
///
/// RFC1918 and CGNAT ranges can contain cluster-internal pod/service IPs.
#[inline]
pub(crate) fn validate_backend_addr(addr: &SocketAddr, allow_loopback_upstream: bool) -> Result<(), EdgionErrorCode> {
    match classify_backend_addr(addr) {
        None => Ok(()),
        Some(BlockedBackendReason::Localhost) if allow_loopback_upstream => Ok(()),
        Some(BlockedBackendReason::Localhost) => Err(EdgionErrorCode::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => Err(EdgionErrorCode::BackendAddrLinkLocalNotAllowed),
        Some(BlockedBackendReason::BlockedCidr) => Err(EdgionErrorCode::BackendAddrBlockedCidrNotAllowed),
        Some(BlockedBackendReason::NonDialable) => Err(EdgionErrorCode::BackendAddrNonDialable),
    }
}

/// Pick the first resolved IP that passes [`validate_backend_addr`] (SSRF), as a
/// `SocketAddr::Inet` bound to `port`.
///
/// A per-request lookup (`system`-mode `ExternalName`, L4 `system` mode) can
/// return several addresses; routing to any that fails the SSRF gate is forbidden, so this
/// skips the unsafe ones and returns the first safe address. If none are safe,
/// it returns the rejection status of the last candidate (a stable
/// loopback/link-local status) so the caller can record it; an empty input
/// yields [`EdgionErrorCode::BackendDnsResolutionFailed`].
pub(crate) fn first_safe_inet_addr(
    ips: &[std::net::IpAddr],
    port: u16,
    allow_loopback_upstream: bool,
) -> Result<SocketAddr, EdgionErrorCode> {
    let mut last_err = EdgionErrorCode::BackendDnsResolutionFailed;
    for ip in ips {
        let addr = SocketAddr::Inet(std::net::SocketAddr::new(*ip, port));
        match validate_backend_addr(&addr, allow_loopback_upstream) {
            Ok(()) => return Ok(addr),
            Err(status) => last_err = status,
        }
    }
    Err(last_err)
}

/// Validate endpoint-derived backend addresses for metadata/link-local safety.
///
/// Loopback is allowed only in explicit integration testing mode because local
/// integration fixtures use 127.0.0.1 endpoints. Production endpoint-derived
/// upstreams reject both loopback and link-local ranges.
#[inline]
pub(crate) fn validate_endpoint_backend_addr(
    addr: &SocketAddr,
    allow_loopback_upstream: bool,
) -> Result<(), EdgionErrorCode> {
    if addr.as_inet().is_none() {
        return Err(EdgionErrorCode::BackendPortResolutionFailed);
    }
    match validate_backend_addr(addr, allow_loopback_upstream) {
        Err(EdgionErrorCode::BackendLocalhostNotAllowed) if edgion_common::test_mode::is_integration_testing_mode() => {
            Ok(())
        }
        result => result,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/backends/peer_builder_test.rs"]
mod tests;
