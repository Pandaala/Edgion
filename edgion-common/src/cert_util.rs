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
mod tests {
    use super::*;
    use ::time::OffsetDateTime;

    /// A minimal self-signed leaf cert PEM with an explicit validity window.
    fn test_cert_pem(start_ts: i64, end_ts: i64) -> String {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params.not_before = OffsetDateTime::from_unix_timestamp(start_ts).unwrap();
        params.not_after = OffsetDateTime::from_unix_timestamp(end_ts).unwrap();
        params.self_signed(&key).unwrap().pem()
    }

    const T0: i64 = 1_800_000_000; // arbitrary fixed epoch base for readable math
    const DAY: i64 = 86_400;

    #[test]
    fn parse_leaf_validity_returns_the_signed_window() {
        let cert = test_cert_pem(T0, T0 + 30 * DAY);
        let v = parse_leaf_validity(&cert).unwrap();
        // rcgen truncates to whole seconds; the window must round-trip exactly.
        assert_eq!(v.not_before, T0);
        assert_eq!(v.not_after, T0 + 30 * DAY);
    }

    #[test]
    fn parse_leaf_validity_rejects_garbage() {
        assert!(parse_leaf_validity("not a pem").is_err());
        assert!(parse_leaf_validity("-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n").is_err());
    }

    #[test]
    fn remaining_seconds_counts_down_and_goes_negative_after_expiry() {
        let v = LeafValidity {
            not_before: T0,
            not_after: T0 + 10 * DAY,
        };
        assert_eq!(v.remaining_seconds(T0), 10 * DAY);
        assert_eq!(v.remaining_seconds(T0 + 10 * DAY + 5), -5);
    }
}
