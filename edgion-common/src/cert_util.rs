//! Certificate lifecycle math shared by the conf_sync expiry safety net and
//! the (future) leaf-rotation reconciler.
//!
//! Pure functions only: parsing a leaf's validity window, deciding whether
//! rotation is due, and validating a candidate cert/key/CA triple before it
//! is swapped into service. No I/O, no clocks — callers pass `now` explicitly
//! so every branch is unit-testable.

use x509_parser::pem::Pem;
use x509_parser::prelude::*;

/// Fraction of the leaf lifetime after which rotation is due (Istio-style
/// half-life renewal; `SECRET_GRACE_PERIOD_RATIO` default equivalent).
pub const ROTATION_LIFETIME_RATIO: f64 = 0.5;

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

    /// True once `now` has passed [`ROTATION_LIFETIME_RATIO`] of the lifetime.
    ///
    /// A degenerate window (`not_after <= not_before`) is always due: it can
    /// only come from a broken issuer or clock, and "rotate now" is the safe
    /// answer in both cases.
    pub fn rotation_due(&self, now: i64) -> bool {
        let lifetime = self.not_after - self.not_before;
        if lifetime <= 0 {
            return true;
        }
        let threshold = self.not_before + (lifetime as f64 * ROTATION_LIFETIME_RATIO) as i64;
        now >= threshold
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

/// Validate a candidate cert/key/CA triple before swapping it into service:
/// the leaf must parse, must be signed by the given CA, and must match the
/// private key. Returns a human-readable reason on failure.
pub fn validate_leaf_swap(cert_pem: &str, key_pem: &str, ca_pem: &str) -> Result<(), String> {
    let leaf_der = first_pem_der(cert_pem).map_err(|e| format!("leaf cert: {e}"))?;
    let (_, leaf) = X509Certificate::from_der(&leaf_der).map_err(|e| format!("leaf cert: X.509 parse error: {e}"))?;

    let ca_der = first_pem_der(ca_pem).map_err(|e| format!("CA cert: {e}"))?;
    let (_, ca) = X509Certificate::from_der(&ca_der).map_err(|e| format!("CA cert: X.509 parse error: {e}"))?;

    leaf.verify_signature(Some(ca.public_key()))
        .map_err(|e| format!("leaf is not signed by the given CA (signature check failed: {e})"))?;

    crate::tls_validation::check_key_matches_cert(cert_pem, key_pem).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::time::OffsetDateTime;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };

    /// Sign a self-contained CA + leaf pair with an explicit validity window.
    fn make_ca_and_leaf(not_before: OffsetDateTime, not_after: OffsetDateTime) -> (String, String, String) {
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut ca_params = CertificateParams::new(vec![]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "test conf_sync root");
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        leaf_params.not_before = not_before;
        leaf_params.not_after = not_after;
        leaf_params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
        let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).unwrap();

        (leaf_cert.pem(), leaf_key.serialize_pem(), ca_cert.pem())
    }

    fn window(start_ts: i64, end_ts: i64) -> (String, String, String) {
        make_ca_and_leaf(
            OffsetDateTime::from_unix_timestamp(start_ts).unwrap(),
            OffsetDateTime::from_unix_timestamp(end_ts).unwrap(),
        )
    }

    const T0: i64 = 1_800_000_000; // arbitrary fixed epoch base for readable math
    const DAY: i64 = 86_400;

    #[test]
    fn parse_leaf_validity_returns_the_signed_window() {
        let (cert, _key, _ca) = window(T0, T0 + 30 * DAY);
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

    #[test]
    fn rotation_due_flips_at_half_lifetime() {
        let v = LeafValidity {
            not_before: T0,
            not_after: T0 + 10 * DAY,
        };
        assert!(!v.rotation_due(T0));
        assert!(!v.rotation_due(T0 + 5 * DAY - 1));
        assert!(v.rotation_due(T0 + 5 * DAY));
        assert!(v.rotation_due(T0 + 20 * DAY)); // long past expiry: still due
    }

    #[test]
    fn rotation_due_treats_degenerate_window_as_due() {
        let v = LeafValidity {
            not_before: T0,
            not_after: T0, // zero-length lifetime
        };
        assert!(v.rotation_due(T0));
        let inverted = LeafValidity {
            not_before: T0 + DAY,
            not_after: T0,
        };
        assert!(inverted.rotation_due(T0));
    }

    #[test]
    fn validate_leaf_swap_accepts_a_matching_triple() {
        let (cert, key, ca) = window(T0, T0 + 30 * DAY);
        assert_eq!(validate_leaf_swap(&cert, &key, &ca), Ok(()));
    }

    #[test]
    fn validate_leaf_swap_rejects_key_from_another_cert() {
        let (cert, _key, ca) = window(T0, T0 + 30 * DAY);
        let (_other_cert, other_key, _other_ca) = window(T0, T0 + 30 * DAY);
        let err = validate_leaf_swap(&cert, &other_key, &ca).unwrap_err();
        assert!(err.contains("key"), "error should mention the key mismatch, got: {err}");
    }

    #[test]
    fn validate_leaf_swap_rejects_leaf_signed_by_a_different_ca() {
        let (cert, key, _ca) = window(T0, T0 + 30 * DAY);
        let (_c2, _k2, unrelated_ca) = window(T0, T0 + 30 * DAY);
        let err = validate_leaf_swap(&cert, &key, &unrelated_ca).unwrap_err();
        assert!(
            err.to_lowercase().contains("ca") || err.to_lowercase().contains("signature"),
            "error should mention the CA/signature mismatch, got: {err}"
        );
    }

    #[test]
    fn validate_leaf_swap_rejects_unparseable_inputs() {
        let (cert, key, ca) = window(T0, T0 + 30 * DAY);
        assert!(validate_leaf_swap("junk", &key, &ca).is_err());
        assert!(validate_leaf_swap(&cert, "junk", &ca).is_err());
        assert!(validate_leaf_swap(&cert, &key, "junk").is_err());
    }
}
