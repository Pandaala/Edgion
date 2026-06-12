//! Shared TLS certificate/key validation helpers.
//!
//! `check_key_matches_cert` is a pure crypto/PEM validator (no proxy runtime
//! state) used by both the control plane (resource ingest validation) and the
//! data plane (TLS config validation). It lives in `common` so the controller
//! does not need to reach into the gateway module for it.

use std::fmt;
use x509_parser::pem::Pem;
use x509_parser::prelude::*;

/// Error returned when a certificate/private-key pair fails to match.
///
/// The `Display` strings intentionally mirror the corresponding
/// `gateway::tls::validation::CertValidationError` variants so that existing
/// warning/error messages are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCertMatchError {
    /// Certificate PEM parsing failed
    CertParse(String),
    /// Private key PEM parsing failed
    KeyParse(String),
    /// Private key does not match certificate public key
    Mismatch(String),
}

impl fmt::Display for KeyCertMatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CertParse(msg) => write!(f, "Certificate parse error: {}", msg),
            Self::KeyParse(msg) => write!(f, "Private key parse error: {}", msg),
            Self::Mismatch(msg) => write!(f, "Key mismatch: {}", msg),
        }
    }
}

impl std::error::Error for KeyCertMatchError {}

/// Verify that a PEM private key matches the public key in a PEM certificate.
pub fn check_key_matches_cert(cert_pem: &str, key_pem: &str) -> Result<(), KeyCertMatchError> {
    let key_pair = rcgen::KeyPair::from_pem(key_pem)
        .map_err(|e| KeyCertMatchError::KeyParse(format!("Failed to parse private key: {}", e)))?;

    let pem = Pem::iter_from_buffer(cert_pem.as_bytes())
        .next()
        .ok_or_else(|| KeyCertMatchError::CertParse("No PEM block found in cert".to_string()))?
        .map_err(|e| KeyCertMatchError::CertParse(format!("Cert PEM parse error: {}", e)))?;

    let (_, cert) = X509Certificate::from_der(&pem.contents)
        .map_err(|e| KeyCertMatchError::CertParse(format!("X.509 parse error: {}", e)))?;
    let cert_pubkey_raw = &cert.public_key().subject_public_key.data;
    let key_pubkey_raw = key_pair.public_key_raw();

    if cert_pubkey_raw.as_ref() != key_pubkey_raw {
        return Err(KeyCertMatchError::Mismatch(
            "Private key does not match certificate public key".to_string(),
        ));
    }

    Ok(())
}
