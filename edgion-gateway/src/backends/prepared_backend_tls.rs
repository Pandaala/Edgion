//! Immutable native TLS preparation owned by configuration generations.

use crate::tls::runtime::backend::cert_extractor::extract_sans_from_ssl;
use crate::tls::validation::backend_san::{validate_backend_sans, BackendSanValidationResult};
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use edgion_resources::resources::backend_tls_policy::SubjectAltName;
use edgion_resources::resources::BackendTLSPolicy;
use edgion_resources::EdgionErrorCode;
use k8s_openapi::api::core::v1::Secret;
use pingora_core::protocols::tls::{CaType, HandshakeCompleteHook};
use pingora_core::tls::{pkey::PKey, x509::X509};
use pingora_core::utils::tls::CertKey;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(super) struct PreparedBackendTls {
    pub identity: Option<Result<Arc<CertKey>, EdgionErrorCode>>,
    pub ca: Result<Option<Arc<CaType>>, EdgionErrorCode>,
    pub san_hook: Option<HandshakeCompleteHook>,
}

impl PreparedBackendTls {
    pub fn new(policy: &BackendTLSPolicy) -> Self {
        Self {
            identity: policy
                .spec
                .resolved_client_certificate
                .as_ref()
                .map(build_client_cert_key_from_secret),
            ca: if policy.spec.use_system_ca {
                Ok(None)
            } else {
                build_ca_chain(policy)
            },
            san_hook: policy
                .spec
                .validation
                .subject_alt_names
                .as_ref()
                .filter(|sans| !sans.is_empty())
                .map(|sans| {
                    let sans = sans.clone();
                    Arc::new(move |ssl: &pingora_core::tls::ssl::SslRef| {
                        Some(Arc::new(evaluate_sans(&sans, || extract_sans_from_ssl(ssl)))
                            as Arc<dyn std::any::Any + Send + Sync>)
                    }) as HandshakeCompleteHook
                }),
        }
    }

    pub fn inputs_equivalent(left: &BackendTLSPolicy, right: &BackendTLSPolicy) -> bool {
        left.spec.use_system_ca == right.spec.use_system_ca
            && left
                .spec
                .resolved_client_certificate
                .as_ref()
                .map(|secret| &secret.data)
                == right
                    .spec
                    .resolved_client_certificate
                    .as_ref()
                    .map(|secret| &secret.data)
            && match (
                &left.spec.resolved_ca_certificates,
                &right.spec.resolved_ca_certificates,
            ) {
                (Some(left), Some(right)) => left
                    .iter()
                    .map(|secret| &secret.data)
                    .eq(right.iter().map(|secret| &secret.data)),
                (None, None) => true,
                _ => false,
            }
            && left.spec.validation.subject_alt_names == right.spec.validation.subject_alt_names
    }
}

pub(super) struct PreparedGatewayIdentity {
    pub identity: Result<Arc<CertKey>, EdgionErrorCode>,
    pub fingerprint: [u8; 32],
}

impl PreparedGatewayIdentity {
    pub fn new(secret: &Secret) -> Self {
        Self {
            identity: build_client_cert_key_from_secret(secret),
            fingerprint: secret_data_fingerprint(secret),
        }
    }
}

fn evaluate_sans(sans: &[SubjectAltName], extract: impl FnOnce() -> Vec<String>) -> BackendSanValidationResult {
    catch_redacted_panic(|| validate_backend_sans(&extract(), sans)).unwrap_or(BackendSanValidationResult {
        passed: false,
        reason: "backend SAN validation panicked",
    })
}

pub(super) fn build_client_cert_key_from_secret(
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

pub(super) fn build_ca_chain(policy: &BackendTLSPolicy) -> Result<Option<Arc<CaType>>, EdgionErrorCode> {
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

#[cfg(test)]
#[path = "../../tests/unit/backends/prepared_backend_tls_test.rs"]
mod tests;
