//! Verified TLS configuration for Gateway outbound HTTP clients.

use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
use edgion_resources::resources::backend_tls_policy::SubjectAltName;
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};

use super::OutboundSanVerifier;

/// Build a verified HTTPS configuration from canonical outbound TLS resolution.
///
/// Custom roots replace public roots. The verifier always checks the CA chain,
/// expiry and URL hostname, then enforces any additional SAN policy. This helper
/// deliberately never disables verification, including when the resolved policy
/// has `verify=false`; callers needing that mode use the common TLS factory.
pub fn build_verified_outbound_tls(
    resolved: &ResolvedOutboundTls,
    san_policy: Vec<SubjectAltName>,
    context: &str,
) -> Result<ClientConfig, String> {
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("{context} TLS: rustls provider error: {e}"))?;

    // Root store
    let mut roots = RootCertStore::empty();
    match &resolved.ca {
        EffectiveCa::System => {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        EffectiveCa::Custom(bundles) => {
            if bundles.is_empty() {
                return Err(format!("{context} TLS: CA PEM contained no valid certificates"));
            }
            for bundle in bundles {
                let mut added = 0usize;
                for cert in CertificateDer::pem_slice_iter(bundle) {
                    let cert = cert.map_err(|e| format!("{context} TLS: invalid CA PEM: {e}"))?;
                    roots
                        .add(cert)
                        .map_err(|e| format!("{context} TLS: add CA failed: {e}"))?;
                    added += 1;
                }
                if added == 0 {
                    return Err(format!("{context} TLS: CA PEM bundle contained no valid certificates"));
                }
            }
        }
    }

    // Wrap webpki verifier with OutboundSanVerifier
    let webpki = WebPkiServerVerifier::builder_with_provider(std::sync::Arc::new(roots), provider)
        .build()
        .map_err(|e| format!("{context} TLS: webpki verifier build error: {e}"))?;
    let san_verifier = std::sync::Arc::new(OutboundSanVerifier::new(webpki, san_policy));
    let builder = builder.dangerous().with_custom_certificate_verifier(san_verifier);

    // Apply the resolved client identity without falling back on invalid material.
    let mut config = match &resolved.client_identity {
        Some((cert_pem, key_pem)) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
                .collect::<Result<_, _>>()
                .map_err(|e| format!("{context} TLS: invalid client cert PEM: {e}"))?;
            if certs.is_empty() {
                return Err(format!("{context} TLS: client cert PEM contained no certificates"));
            }
            let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
                .map_err(|e| format!("{context} TLS: invalid client key PEM: {e}"))?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| format!("{context} TLS: client auth cert error: {e}"))?
        }
        None => builder.with_no_client_auth(),
    };

    // HTTPS clients may negotiate HTTP/2.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(config)
}

#[cfg(test)]
#[path = "../../../tests/unit/tls/validation/outbound_client_test.rs"]
mod tests;
