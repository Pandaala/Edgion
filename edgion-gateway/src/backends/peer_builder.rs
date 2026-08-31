use crate::backends::discovery::get_global_service_store;
#[cfg(any(feature = "boringssl", test))]
use crate::backends::gateway_backend_tls_store::get_global_gateway_backend_tls_store;
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
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendAppProtocol {
    H2c,
    WebSocket,
}

fn parse_backend_app_protocol(value: &str) -> Option<BackendAppProtocol> {
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

pub(crate) fn detect_backend_app_protocol_key(
    service_key: &str,
    backend_port: Option<i32>,
) -> Option<BackendAppProtocol> {
    let service = get_global_service_store().get(service_key)?;
    let ports = service.spec.as_ref()?.ports.as_ref()?;

    let target_port = backend_port.or_else(|| ports.first().map(|p| p.port))?;
    let port = ports.iter().find(|p| p.port == target_port).or_else(|| ports.first())?;
    parse_backend_app_protocol(port.app_protocol.as_deref()?)
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

pub(crate) fn create_tls_peer(
    addr: SocketAddr,
    policy: &Option<Arc<BackendTLSPolicy>>,
    gateway_key: Option<&str>,
) -> Result<Box<HttpPeer>, EdgionErrorCode> {
    let Some(policy) = policy.as_ref() else {
        return Ok(Box::new(HttpPeer::new(addr, false, String::new())));
    };

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
    let peer = {
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
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::Secret;
    use std::net::IpAddr;
    use std::sync::Arc;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn first_safe_inet_addr_skips_unsafe_and_picks_public() {
        // Loopback + link-local are unsafe; the public IP is selected.
        let ips = [ip("127.0.0.1"), ip("169.254.169.254"), ip("93.184.216.34")];
        let addr = first_safe_inet_addr(&ips, 443, false).unwrap();
        match addr {
            SocketAddr::Inet(a) => {
                assert_eq!(a.ip(), ip("93.184.216.34"));
                assert_eq!(a.port(), 443);
            }
            _ => panic!("expected inet addr"),
        }
    }

    #[test]
    fn first_safe_inet_addr_all_unsafe_errors() {
        let ips = [ip("127.0.0.1"), ip("169.254.169.254")];
        let err = first_safe_inet_addr(&ips, 80, false).unwrap_err();
        // The last candidate's rejection status surfaces (link-local here).
        assert_eq!(err, EdgionErrorCode::BackendAddrLinkLocalNotAllowed);
    }

    #[test]
    fn first_safe_inet_addr_loopback_allowed_when_flag_set() {
        let ips = [ip("127.0.0.1")];
        let addr = first_safe_inet_addr(&ips, 8080, true).unwrap();
        assert!(matches!(addr, SocketAddr::Inet(_)));
    }

    #[test]
    fn first_safe_inet_addr_empty_is_dns_failure() {
        let err = first_safe_inet_addr(&[], 80, false).unwrap_err();
        assert_eq!(err, EdgionErrorCode::BackendDnsResolutionFailed);
    }

    #[test]
    fn client_identity_requires_policy_and_prefers_policy_identity() {
        use ClientIdentitySource::{Gateway, Policy};

        let cases = [
            (false, false, false, None),
            (false, false, true, None),
            (false, true, false, None),
            (false, true, true, None),
            (true, false, false, None),
            (true, false, true, Some(Gateway)),
            (true, true, false, Some(Policy)),
            (true, true, true, Some(Policy)),
        ];

        for (policy_present, policy_identity_present, gateway_identity_present, expected) in cases {
            assert_eq!(
                select_client_identity_source(policy_present, policy_identity_present, gateway_identity_present),
                expected
            );
        }
    }

    #[test]
    fn backend_app_protocol_aliases_are_ascii_case_insensitive() {
        let cases = [
            ("KuBeRnEtEs.Io/H2C", Some(BackendAppProtocol::H2c)),
            ("H2C", Some(BackendAppProtocol::H2c)),
            ("KuBeRnEtEs.Io/Ws", Some(BackendAppProtocol::WebSocket)),
            ("Ws", Some(BackendAppProtocol::WebSocket)),
            ("KuBeRnEtEs.Io/WsS", Some(BackendAppProtocol::WebSocket)),
            ("WsS", Some(BackendAppProtocol::WebSocket)),
            ("KuBeRnEtEs.Io/Unknown", None),
        ];

        for (value, expected) in cases {
            assert_eq!(parse_backend_app_protocol(value), expected, "value={value}");
        }
    }

    #[cfg(feature = "boringssl")]
    fn client_identity_secret(cert: Option<&str>, key: Option<&str>) -> Secret {
        let mut data = std::collections::BTreeMap::new();
        if let Some(cert) = cert {
            data.insert(CERT.to_string(), k8s_openapi::ByteString(cert.as_bytes().to_vec()));
        }
        if let Some(key) = key {
            data.insert(KEY.to_string(), k8s_openapi::ByteString(key.as_bytes().to_vec()));
        }
        Secret {
            data: Some(data),
            ..Default::default()
        }
    }

    #[cfg(feature = "boringssl")]
    fn generated_cert_and_key(name: &str) -> (String, String) {
        let key = rcgen::KeyPair::generate().expect("test key generation must succeed");
        let params =
            rcgen::CertificateParams::new(vec![name.to_string()]).expect("test certificate parameters must be valid");
        let cert = params.self_signed(&key).expect("test certificate signing must succeed");
        (cert.pem(), key.serialize_pem())
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn client_identity_preserves_single_and_multi_certificate_chains() {
        let (leaf_pem, leaf_key) = generated_cert_and_key("leaf.example");
        let (intermediate_pem, _) = generated_cert_and_key("intermediate.example");

        let single = build_client_cert_key_from_secret(&client_identity_secret(Some(&leaf_pem), Some(&leaf_key)))
            .expect("single certificate identity must parse");
        assert!(single.intermediates().is_empty());

        let bundle = format!("{leaf_pem}{intermediate_pem}");
        let chain = build_client_cert_key_from_secret(&client_identity_secret(Some(&bundle), Some(&leaf_key)))
            .expect("certificate chain identity must parse");
        assert_eq!(chain.intermediates().len(), 1);

        let expected_leaf = X509::from_pem(leaf_pem.as_bytes()).expect("leaf certificate must parse");
        let expected_intermediate =
            X509::from_pem(intermediate_pem.as_bytes()).expect("intermediate certificate must parse");
        assert_eq!(
            chain.leaf().to_der().expect("leaf DER"),
            expected_leaf.to_der().expect("expected leaf DER")
        );
        assert_eq!(
            chain.intermediates()[0].to_der().expect("intermediate DER"),
            expected_intermediate.to_der().expect("expected intermediate DER")
        );
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn client_identity_rejects_missing_empty_and_malformed_material() {
        let (cert_pem, key_pem) = generated_cert_and_key("client.example");
        let cases = [
            Secret::default(),
            client_identity_secret(None, Some(&key_pem)),
            client_identity_secret(Some(&cert_pem), None),
            client_identity_secret(Some(""), Some(&key_pem)),
            client_identity_secret(Some("not a certificate"), Some(&key_pem)),
            client_identity_secret(Some(&cert_pem), Some("")),
            client_identity_secret(Some(&cert_pem), Some("not a private key")),
        ];

        for secret in cases {
            assert!(matches!(
                build_client_cert_key_from_secret(&secret),
                Err(EdgionErrorCode::Unknown)
            ));
        }
    }

    #[test]
    fn gateway_identity_does_not_enable_tls_without_policy() {
        let gateway_key = "test/gateway-identity-without-policy";
        let store = get_global_gateway_backend_tls_store();
        store.set(gateway_key.to_string(), Arc::new(Secret::default()));

        let result = create_tls_peer(
            SocketAddr::Inet("127.0.0.1:443".parse().unwrap()),
            &None,
            Some(gateway_key),
        );
        store.remove(gateway_key);

        let peer = result.expect("Gateway identity must not be parsed without a BackendTLSPolicy");
        assert!(!peer.is_tls());
        assert!(peer.client_cert_key.is_none());
    }
}
