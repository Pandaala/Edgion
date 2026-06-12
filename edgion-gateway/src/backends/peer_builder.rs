use crate::backends::discovery::get_global_service_store;
use crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store;
use crate::EdgionHttpContext;
use edgion_common::utils::net::{classify_backend_addr, BlockedBackendReason};
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::edgion_status::EdgionStatus;
use edgion_resources::resources::BackendTLSPolicy;
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
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendAppProtocol {
    H2c,
    WebSocket,
}

pub(crate) fn detect_backend_app_protocol_key(
    service_key: &str,
    backend_port: Option<i32>,
) -> Option<BackendAppProtocol> {
    let service = get_global_service_store().get(service_key)?;
    let ports = service.spec.as_ref()?.ports.as_ref()?;

    let target_port = backend_port.or_else(|| ports.first().map(|p| p.port))?;
    let port = ports.iter().find(|p| p.port == target_port).or_else(|| ports.first())?;
    let app_protocol = port.app_protocol.as_deref()?.to_ascii_lowercase();

    match app_protocol.as_str() {
        "kubernetes.io/h2c" | "h2c" => Some(BackendAppProtocol::H2c),
        "kubernetes.io/ws" | "ws" | "kubernetes.io/wss" | "wss" => Some(BackendAppProtocol::WebSocket),
        _ => None,
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
            peer.options.set_http_version(1, 1);
            if ctx.request_info.discover_protocol.is_none() {
                ctx.request_info.discover_protocol = Some("websocket".to_string());
            }
        }
        None => {}
    }
}

/// Extract TLS configuration from BackendTLSPolicy
///
/// Returns (use_tls, sni) tuple:
/// - If policy exists: (true, hostname from policy)
/// - Otherwise: (false, empty string)
#[inline]
pub(crate) fn extract_tls_config(policy: &Option<Arc<BackendTLSPolicy>>) -> (bool, String) {
    policy
        .as_ref()
        .map(|p| (true, p.spec.validation.hostname.clone()))
        .unwrap_or((false, String::new()))
}

#[cfg(feature = "boringssl")]
fn build_client_cert_key(policy: &BackendTLSPolicy) -> Result<Option<Arc<CertKey>>, EdgionStatus> {
    let Some(secret) = &policy.spec.resolved_client_certificate else {
        return Ok(None);
    };

    build_client_cert_key_from_secret(secret)
}

#[cfg(feature = "boringssl")]
fn build_client_cert_key_from_secret(
    secret: &k8s_openapi::api::core::v1::Secret,
) -> Result<Option<Arc<CertKey>>, EdgionStatus> {
    let data = secret.data.as_ref().ok_or(EdgionStatus::Unknown)?;
    let cert_pem = data.get(CERT).ok_or(EdgionStatus::Unknown)?;
    let key_pem = data.get(KEY).ok_or(EdgionStatus::Unknown)?;

    let cert = X509::from_pem(cert_pem.0.as_slice()).map_err(|_| EdgionStatus::Unknown)?;
    let key = PKey::private_key_from_pem(key_pem.0.as_slice()).map_err(|_| EdgionStatus::Unknown)?;

    Ok(Some(Arc::new(CertKey::new(vec![cert], key))))
}

#[cfg(feature = "boringssl")]
fn build_ca_chain(policy: &BackendTLSPolicy) -> Result<Option<Arc<CaType>>, EdgionStatus> {
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
        let certs = X509::stack_from_pem(ca_pem.0.as_slice()).map_err(|_| EdgionStatus::BackendTlsCaInvalid)?;
        if certs.is_empty() {
            return Err(EdgionStatus::BackendTlsCaInvalid);
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
fn build_ca_chain(_policy: &BackendTLSPolicy) -> Result<Option<Arc<()>>, EdgionStatus> {
    Ok(None)
}

pub(crate) fn create_tls_peer(
    addr: SocketAddr,
    policy: &Option<Arc<BackendTLSPolicy>>,
    gateway_key: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionStatus> {
    let Some(policy) = policy.as_ref() else {
        if let Some(gw_key) = gateway_key {
            if let Some(gw_secret) = get_global_gateway_backend_tls_store().get(gw_key) {
                #[cfg(feature = "boringssl")]
                if let Some(cert_key) = build_client_cert_key_from_secret(&gw_secret)? {
                    let peer = HttpPeer::new_mtls(addr, String::new(), cert_key);
                    return Ok(Box::new(peer));
                }

                #[cfg(not(feature = "boringssl"))]
                let _ = gw_secret;
            }
        }
        return Ok(Box::new(HttpPeer::new(addr, false, String::new())));
    };

    let sni = policy.spec.validation.hostname.clone();

    #[cfg(feature = "boringssl")]
    let mut peer = if let Some(cert_key) = build_client_cert_key(policy)? {
        HttpPeer::new_mtls(addr, sni.clone(), cert_key)
    } else if let Some(gw_key) = gateway_key {
        if let Some(gw_secret) = get_global_gateway_backend_tls_store().get(gw_key) {
            if let Some(cert_key) = build_client_cert_key_from_secret(&gw_secret)? {
                HttpPeer::new_mtls(addr, sni.clone(), cert_key)
            } else {
                HttpPeer::new(addr, true, sni)
            }
        } else {
            HttpPeer::new(addr, true, sni)
        }
    } else {
        HttpPeer::new(addr, true, sni)
    };

    #[cfg(not(feature = "boringssl"))]
    let mut peer = HttpPeer::new(addr, true, sni);

    #[cfg(feature = "boringssl")]
    if !policy.spec.use_system_ca {
        // Operator opted out of the system trust store, so a missing/empty CA
        // bundle must fail-closed rather than silently fall back to it.
        match build_ca_chain(policy)? {
            Some(ca_chain) => peer.options.ca = Some(ca_chain),
            None => return Err(EdgionStatus::BackendTlsCaInvalid),
        }
    }

    #[cfg(feature = "boringssl")]
    if let Some(ref policy_sans) = policy.spec.validation.subject_alt_names {
        if !policy_sans.is_empty() {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for san in policy_sans.iter() {
                match san.san_type {
                    edgion_resources::resources::backend_tls_policy::SubjectAltNameType::Hostname => {
                        "H".hash(&mut hasher);
                        san.hostname.hash(&mut hasher);
                    }
                    edgion_resources::resources::backend_tls_policy::SubjectAltNameType::URI => {
                        "U".hash(&mut hasher);
                        san.uri.hash(&mut hasher);
                    }
                }
            }
            peer.group_key = hasher.finish();

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

    Ok(Box::new(peer))
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
pub(crate) fn validate_backend_addr(addr: &SocketAddr, allow_loopback_upstream: bool) -> Result<(), EdgionStatus> {
    match classify_backend_addr(addr) {
        None => Ok(()),
        Some(BlockedBackendReason::Localhost) if allow_loopback_upstream => Ok(()),
        Some(BlockedBackendReason::Localhost) => Err(EdgionStatus::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => Err(EdgionStatus::BackendAddrLinkLocalNotAllowed),
    }
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
) -> Result<(), EdgionStatus> {
    match classify_backend_addr(addr) {
        Some(BlockedBackendReason::Localhost)
            if allow_loopback_upstream || edgion_common::test_mode::is_integration_testing_mode() =>
        {
            Ok(())
        }
        Some(BlockedBackendReason::Localhost) => Err(EdgionStatus::BackendLocalhostNotAllowed),
        Some(BlockedBackendReason::LinkLocal) => Err(EdgionStatus::BackendAddrLinkLocalNotAllowed),
        None => Ok(()),
    }
}
