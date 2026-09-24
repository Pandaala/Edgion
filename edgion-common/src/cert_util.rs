//! Certificate parsing shared by the conf_sync expiry watch
//! (`crate::conf_sync::cert_expiry_watch`).
//!
//! Pure functions only: parsing a leaf's validity window. No I/O, no clocks —
//! callers pass `now` explicitly so every branch is unit-testable. The
//! leaf-rotation reconciler lives in the controller
//! (`edgion-controller/src/services/conf_sync_ca/rotation.rs`) and has its own
//! rotation-due math (`edgion-controller/src/services/conf_sync_ca/cert_util.rs`),
//! not this module.

use x509_parser::pem::Pem;
use x509_parser::prelude::*;

/// A leaf certificate's validity window (Unix timestamps, seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeafValidity {
    pub not_before: i64,
    pub not_after: i64,
}

impl LeafValidity {
    /// Seconds until `not_after` as seen from `now`. Negative once expired.
    pub fn remaining_seconds(&self, now: i64) -> i64 {
        self.not_after - now
    }
}

/// Extract the DER contents of the first PEM block in `pem_str`.
fn first_pem_der(pem_str: &str) -> Result<Vec<u8>, String> {
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()
        .ok_or_else(|| "no PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {e}"))?;
    Ok(pem.contents)
}

/// Parse the FIRST certificate in a PEM bundle and return its validity window.
pub fn parse_leaf_validity(cert_pem: &str) -> Result<LeafValidity, String> {
    let der = first_pem_der(cert_pem)?;
    let (_, cert) = X509Certificate::from_der(&der).map_err(|e| format!("X.509 parse error: {e}"))?;
    Ok(LeafValidity {
        not_before: cert.validity().not_before.timestamp(),
        not_after: cert.validity().not_after.timestamp(),
    })
}

#[cfg(test)]
#[path = "../tests/unit/cert_util_test.rs"]
mod tests;
