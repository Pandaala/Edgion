#[cfg(any(feature = "boringssl", test))]
use crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store;
use crate::backends::policy::backend_tls::backend_tls_policy_store::BACKEND_TLS_PROFILE_DOMAIN;
use crate::backends::policy::{BackendTlsPolicyLookup, BackendTlsProfileIdentity, EffectiveBackendTlsPolicy};
use crate::util::net::{classify_backend_addr, BlockedBackendReason};
use crate::EdgionHttpContext;
#[cfg(feature = "boringssl")]
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::resources::BackendTLSPolicy;
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr;
#[cfg(feature = "boringssl")]
use pingora_core::protocols::tls::CaType;
#[cfg(feature = "boringssl")]
use pingora_core::tls::pkey::PKey;
#[cfg(feature = "boringssl")]
use pingora_core::tls::x509::X509;
#[cfg(feature = "boringssl")]
use pingora_core::utils::tls::CertKey;
use sha2::{Digest, Sha256};
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
                ctx.request_info.discover_protocol = Some("websocket".to_string());
            }
        }
        None => {}
    }
}

/// Extract TLS configuration from a usable BackendTLSPolicy lookup.
///
/// `Invalid` is rejected before this helper is called.
#[inline]
pub(crate) fn extract_tls_config(lookup: &BackendTlsPolicyLookup) -> (bool, String) {
    match lookup {
        BackendTlsPolicyLookup::Effective(effective) => (true, effective.policy.spec.validation.hostname.clone()),
        BackendTlsPolicyLookup::Absent => (false, String::new()),
        BackendTlsPolicyLookup::Invalid => (false, String::new()),
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

#[cfg(feature = "boringssl")]
fn build_client_cert_key_from_secret(
    secret: &k8s_openapi::api::core::v1::Secret,
) -> Result<Arc<CertKey>, EdgionErrorCode> {
    let data = secret.data.as_ref().ok_or(EdgionErrorCode::Unknown)?;
    let cert_pem = data.get(CERT).ok_or(EdgionErrorCode::Unknown)?;
    let key_pem = data.get(KEY).ok_or(EdgionErrorCode::Unknown)?;

    let certs = X509::stack_from_pem(cert_pem.0.as_slice()).map_err(|_| EdgionErrorCode::Unknown)?;
    if certs.is_empty() {
        return Err(EdgionErrorCode::Unknown);
    }
    let key = PKey::private_key_from_pem(key_pem.0.as_slice()).map_err(|_| EdgionErrorCode::Unknown)?;

    Ok(Arc::new(CertKey::new(certs, key)))
}

#[cfg(feature = "boringssl")]
fn build_ca_chain(policy: &BackendTLSPolicy) -> Result<Option<Arc<CaType>>, EdgionErrorCode> {
    let Some(secrets) = policy.spec.resolved_ca_certificates.as_ref() else {
        return Ok(None);
    };
    let mut ca_chain = Vec::new();
    for secret in secrets {
        let Some(data) = &secret.data else {
            continue;
        };
        let Some(ca_pem) = data.get(CA_CERT) else {
            continue;
        };
        // `stack_from_pem` parses every block of a `root + intermediate` bundle
        // (RFC 5280 §6.1 needs the full chain); `from_pem` would keep only the
        // first. A present-but-unparseable/empty `ca.crt` is fail-closed instead
        // of silently swallowed.
        let certs = X509::stack_from_pem(ca_pem.0.as_slice()).map_err(|_| EdgionErrorCode::BackendTlsCaInvalid)?;
        if certs.is_empty() {
            return Err(EdgionErrorCode::BackendTlsCaInvalid);
        }
        ca_chain.extend(certs);
    }

    if ca_chain.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Arc::new(ca_chain.into_boxed_slice())))
    }
}

#[cfg(not(feature = "boringssl"))]
fn build_ca_chain(_policy: &BackendTLSPolicy) -> Result<Option<Arc<()>>, EdgionErrorCode> {
    Ok(None)
}

pub(crate) fn peer_from_backend_tls_lookup(
    addr: SocketAddr,
    lookup: &BackendTlsPolicyLookup,
    gateway_key: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
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
    let sni = policy.spec.validation.hostname.clone();

    #[cfg(feature = "boringssl")]
    let policy_identity_present = policy.spec.resolved_client_certificate.is_some();

    #[cfg(feature = "boringssl")]
    let gateway_identity = if policy_identity_present {
        None
    } else {
        gateway_key.and_then(|key| get_global_gateway_backend_tls_store().get(key))
    };

    #[cfg(feature = "boringssl")]
    let mut peer = match select_client_identity_source(true, policy_identity_present, gateway_identity.is_some()) {
        Some(ClientIdentitySource::Policy) => {
            let secret = policy
                .spec
                .resolved_client_certificate
                .as_ref()
                .ok_or(EdgionErrorCode::Unknown)?;
            let cert_key = build_client_cert_key_from_secret(secret)?;
            HttpPeer::new_mtls(addr, sni, cert_key)
        }
        Some(ClientIdentitySource::Gateway) => {
            let secret = gateway_identity.as_ref().ok_or(EdgionErrorCode::Unknown)?;
            let cert_key = build_client_cert_key_from_secret(secret)?;
            HttpPeer::new_mtls(addr, sni, cert_key)
        }
        None => HttpPeer::new(addr, true, sni),
    };

    #[cfg(not(feature = "boringssl"))]
    let mut peer = {
        let _ = gateway_key;
        HttpPeer::new(addr, true, sni)
    };

    #[cfg(feature = "boringssl")]
    if !policy.spec.use_system_ca {
        // Operator opted out of the system trust store, so a missing/empty CA
        // bundle must fail-closed rather than silently fall back to it.
        match build_ca_chain(policy)? {
            Some(ca_chain) => peer.options.ca = Some(ca_chain),
            None => return Err(EdgionErrorCode::BackendTlsCaInvalid),
        }
    }

    #[cfg(feature = "boringssl")]
    {
        peer.group_key = backend_tls_group_key(&effective.profile, gateway_identity.as_deref());
        if let Some(ref policy_sans) = policy.spec.validation.subject_alt_names {
            if !policy_sans.is_empty() {
                let policy_sans = policy_sans.clone();
                peer.options.upstream_tls_handshake_complete_hook = Some(std::sync::Arc::new(
                    move |ssl: &pingora_core::tls::ssl::SslRef| -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {
                        let cert_sans = crate::tls::runtime::backend::cert_extractor::extract_sans_from_ssl(ssl);
                        let result = crate::tls::validation::backend_san::validate_backend_sans(&cert_sans, &policy_sans);
                        Some(std::sync::Arc::new(result))
                    },
                ));
            }
        }
    }

    #[cfg(not(feature = "boringssl"))]
    {
        peer.group_key = effective.profile.group_key();
    }

    Ok(Box::new(peer))
}

fn backend_tls_group_key(
    profile: &BackendTlsProfileIdentity,
    gateway_secret: Option<&k8s_openapi::api::core::v1::Secret>,
) -> u64 {
    let Some(secret) = gateway_secret else {
        return profile.group_key();
    };
    let mut digest = Sha256::new();
    digest.update(BACKEND_TLS_PROFILE_DOMAIN);
    digest.update(profile.fingerprint);
    let secret_fp = secret_data_fingerprint(secret);
    digest.update((secret_fp.len() as u64).to_be_bytes());
    digest.update(secret_fp);
    let out: [u8; 32] = digest.finalize().into();
    u64::from_be_bytes(out[..8].try_into().expect("sha256 is 32 bytes"))
}

fn secret_data_fingerprint(secret: &k8s_openapi::api::core::v1::Secret) -> [u8; 32] {
    let mut digest = Sha256::new();
    if let Some(data) = secret.data.as_ref() {
        let mut keys: Vec<_> = data.keys().collect();
        keys.sort();
        digest.update((keys.len() as u64).to_be_bytes());
        for key in keys {
            digest.update((key.len() as u64).to_be_bytes());
            digest.update(key.as_bytes());
            let value = &data[key].0;
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
    } else {
        digest.update(0u64.to_be_bytes());
    }
    digest.finalize().into()
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
///
/// RFC1918 private ranges are **not** blocked; cluster-internal pod/service IPs live there.
#[inline]
pub(crate) fn validate_backend_addr(addr: &SocketAddr, allow_loopback_upstream: bool) -> Result<(), EdgionErrorCode> {
    match classify_backend_addr(addr) {
        None => Ok(()),
        Some(BlockedBackendReason::Localhost) if allow_loopback_upstream => Ok(()),
        Some(BlockedBackendReason::Localhost) => Err(EdgionErrorCode::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => Err(EdgionErrorCode::BackendAddrLinkLocalNotAllowed),
    }
}

/// Pick the first resolved IP that passes [`validate_backend_addr`] (SSRF), as a
/// `SocketAddr::Inet` bound to `port`.
///
/// Self-resolution (an `ExternalName` DNS/httpdns lookup) can return several
/// addresses; routing to any that fails the SSRF gate is forbidden, so this
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
    match classify_backend_addr(addr) {
        Some(BlockedBackendReason::Localhost)
            if allow_loopback_upstream || edgion_common::test_mode::is_integration_testing_mode() =>
        {
            Ok(())
        }
        Some(BlockedBackendReason::Localhost) => Err(EdgionErrorCode::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => Err(EdgionErrorCode::BackendAddrLinkLocalNotAllowed),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/backends/peer_builder_test.rs"]
mod tests;
