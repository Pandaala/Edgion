//! Certificate validity shared by conf_sync and compiled serving identities.
//!
//! Pure functions only: parsing a leaf's validity window. No I/O, no clocks —
//! callers pass `now` explicitly so every branch is unit-testable. The
//! leaf-rotation reconciler lives in the controller
//! (`edgion-controller/src/services/conf_sync_ca/rotation.rs`) and has its own
//! rotation-due math (`edgion-controller/src/services/conf_sync_ca/cert_util.rs`),
//! not this module.

use rustls::pki_types::CertificateDer;
use std::io::Cursor;
use x509_parser::prelude::*;

/// A leaf certificate's validity window (Unix timestamps, seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeafValidity {
    pub not_before: i64,
    pub not_after: i64,
}

impl LeafValidity {
    /// Extract the signed bounds without retaining certificate material.
    pub fn from_certificate(cert: &X509Certificate<'_>) -> Self {
        Self {
            not_before: cert.validity().not_before.timestamp(),
            not_after: cert.validity().not_after.timestamp(),
        }
    }

    /// RFC 5280 includes both validity bounds. Static diagnostics are safe on
    /// the handshake path and carry no certificate material.
    pub fn validity_error(&self, now: i64) -> Option<&'static str> {
        if now < self.not_before {
            Some("cert_not_yet_valid")
        } else if now > self.not_after {
            Some("cert_expired")
        } else {
            None
        }
    }

    /// Seconds until `not_after` as seen from `now`. Negative once expired.
    pub fn remaining_seconds(&self, now: i64) -> i64 {
        self.not_after.saturating_sub(now)
    }
}

/// Parse the first CERTIFICATE block using the same selection as tonic's
/// rustls identity loader. Other PEM labels cannot substitute their validity.
pub fn parse_leaf_validity(cert_pem: impl AsRef<[u8]>) -> Result<LeafValidity, String> {
    let der = first_certificate_der(cert_pem.as_ref())?;
    let (_, cert) = X509Certificate::from_der(der.as_ref()).map_err(|e| format!("X.509 parse error: {e}"))?;
    Ok(LeafValidity::from_certificate(&cert))
}

pub(crate) fn first_certificate_der(cert_pem: &[u8]) -> Result<CertificateDer<'static>, String> {
    rustls_pemfile::certs(&mut Cursor::new(cert_pem))
        .next()
        .ok_or_else(|| "no CERTIFICATE block found".to_string())?
        // rustls-pemfile errors may include the original BEGIN line or an
        // input-derived END marker. Never forward certificate input to logs.
        .map_err(|_| "invalid certificate PEM".to_string())
}

#[cfg(test)]
#[path = "../tests/unit/cert_util_test.rs"]
mod tests;
