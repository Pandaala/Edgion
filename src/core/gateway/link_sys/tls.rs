//! Gateway-side TLS material resolution for `LinkSys` resources.
//!
//! Each provider (Redis / Etcd / Elasticsearch / Webhook) used to build its own
//! TLS config from the per-resource `*Tls` struct. After R1 these are all
//! unified behind [`crate::types::resources::common::TlsConfig`]; this module
//! centralizes the lookup / merge logic so every provider consumes the same
//! [`TlsMaterial`] shape.
//!
//! Precedence for each field:
//! - `ca_pem` / `client_cert_pem`: **inline PEM** on the CRD wins if set, else
//!   falls back to the parsed Secret. Rationale: public material, inline is
//!   sometimes convenient for testing and users may mix styles.
//! - `client_key_pem`: **Secret only**. Per D2 the private key cannot be inline
//!   (`deny_unknown_fields` on `TlsCerts` already rejects `clientKey`). If the
//!   Secret is missing `tls.key`, `client_key_pem` stays `None`.

use anyhow::{Context, Result};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use zeroize::Zeroizing;

use crate::types::credentials::{SecretMaterialResolver, TlsCertsResolver, TlsMaterial};
use crate::types::resources::common::TlsConfig;

/// Produce a [`TlsMaterial`] from a CRD-level [`TlsConfig`], merging inline
/// fields with the resolved Kubernetes Secret.
///
/// Returns `None` when the `certs` sub-object is absent or when nothing would
/// populate any of the three material fields — this lets callers treat "no
/// material" uniformly without juggling empty `TlsMaterial` instances.
pub fn build_material(tls: &TlsConfig) -> Option<TlsMaterial> {
    let certs = tls.certs.as_ref()?;

    let mut material = certs
        .secret
        .as_ref()
        .and_then(|s| s.data.as_ref())
        .and_then(|d| TlsCertsResolver.parse(d).ok())
        .unwrap_or(TlsMaterial {
            ca_pem: None,
            client_cert_pem: None,
            client_key_pem: None,
        });

    if let Some(ca) = &certs.ca_cert {
        material.ca_pem = Some(ca.as_bytes().to_vec());
    }
    if let Some(cert) = &certs.client_cert {
        material.client_cert_pem = Some(cert.as_bytes().to_vec());
    }

    if material.ca_pem.is_none() && material.client_cert_pem.is_none() && material.client_key_pem.is_none() {
        None
    } else {
        Some(material)
    }
}

/// Build a rustls [`ClientConfig`] from a CRD-level [`TlsConfig`].
///
/// Used by providers whose SDK consumes `rustls::ClientConfig` directly (e.g.
/// fred's Redis client via `TlsConnector::from(ClientConfig)`). Providers that
/// accept raw PEM (etcd-client / reqwest) should keep using
/// [`build_material`] + their native APIs instead.
///
/// Returns `Ok(None)` when `build_material` yields nothing (CRD has no inline
/// or Secret-sourced TLS material); callers can then fall back to the
/// library's default TLS connector.
///
/// Root certificate selection:
/// - If `ca_pem` is present → the CA is the **only** trust root (private CA
///   mode; the system trust store is intentionally not merged in).
/// - Otherwise → falls back to `rustls-native-certs` so mTLS against a
///   public-CA backend still verifies the server chain.
///
/// Client authentication: applied only when **both** `client_cert_pem` and
/// `client_key_pem` are present; otherwise `.with_no_client_auth()`.
pub fn build_rustls_client_config(tls: &TlsConfig) -> Result<Option<ClientConfig>> {
    let Some(material) = build_material(tls) else {
        return Ok(None);
    };

    let mut roots = RootCertStore::empty();
    if let Some(ca_pem) = &material.ca_pem {
        let mut added = 0usize;
        for cert in CertificateDer::pem_slice_iter(ca_pem) {
            let cert = cert.context("failed to parse CA certificate PEM from LinkSys TLS material")?;
            roots
                .add(cert)
                .context("failed to add CA certificate to rustls RootCertStore")?;
            added += 1;
        }
        if added == 0 {
            anyhow::bail!("LinkSys TLS CA PEM did not contain any valid certificates");
        }
    } else {
        let native = rustls_native_certs::load_native_certs();
        if !native.errors.is_empty() {
            tracing::warn!(
                errors = ?native.errors,
                "LinkSys TLS: some system CA certificates could not be loaded"
            );
        }
        if native.certs.is_empty() {
            anyhow::bail!("no system CA certificates were loaded for LinkSys TLS");
        }
        let mut added = 0usize;
        let mut rejected = 0usize;
        for cert in native.certs {
            if roots.add(cert).is_ok() {
                added += 1;
            } else {
                rejected += 1;
            }
        }
        if rejected > 0 {
            tracing::warn!(
                added,
                rejected,
                "LinkSys TLS: some system CA certificates were rejected by rustls"
            );
        }
        if added == 0 {
            anyhow::bail!("no system CA certificates accepted by rustls for LinkSys TLS");
        }
    }

    let builder = ClientConfig::builder().with_root_certificates(roots);

    let client_config = match (&material.client_cert_pem, &material.client_key_pem) {
        (Some(cert_pem), Some(key_pem)) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
                .collect::<std::result::Result<_, _>>()
                .context("failed to parse client certificate PEM from LinkSys TLS material")?;
            if certs.is_empty() {
                anyhow::bail!("LinkSys TLS client certificate PEM did not contain any certificates");
            }
            let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
                .context("failed to parse client private key PEM from LinkSys TLS Secret")?;
            builder
                .with_client_auth_cert(certs, key)
                .context("failed to configure rustls client authentication")?
        }
        _ => builder.with_no_client_auth(),
    };

    Ok(Some(client_config))
}

/// Convenience: concatenate `client_cert_pem` + newline + `client_key_pem` for
/// libraries (reqwest / hyper-rustls) that accept an mTLS identity as a single
/// combined PEM blob. Returns `None` if either half is missing.
pub fn combined_identity_pem(m: &TlsMaterial) -> Option<Zeroizing<Vec<u8>>> {
    let cert = m.client_cert_pem.as_deref()?;
    let key = m.client_key_pem.as_ref()?;
    let mut out = Vec::with_capacity(cert.len() + key.len() + 1);
    out.extend_from_slice(cert);
    out.push(b'\n');
    out.extend_from_slice(key);
    Some(Zeroizing::new(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::common::{TlsCerts, TlsConfig};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    fn secret_with(data: &[(&str, &[u8])]) -> Secret {
        let data = data
            .iter()
            .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
            .collect::<BTreeMap<_, _>>();
        Secret {
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn returns_none_without_certs() {
        let tls = TlsConfig {
            enabled: true,
            certs: None,
            insecure_skip_verify: None,
        };
        assert!(build_material(&tls).is_none());
    }

    #[test]
    fn extracts_from_secret_only() {
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: None,
                client_cert: None,
                secret_ref: None,
                secret: Some(secret_with(&[
                    ("ca.crt", b"CA"),
                    ("tls.crt", b"CERT"),
                    ("tls.key", b"KEY"),
                ])),
            }),
            insecure_skip_verify: None,
        };
        let m = build_material(&tls).expect("material");
        assert_eq!(m.ca_pem.as_deref(), Some(b"CA".as_ref()));
        assert_eq!(m.client_cert_pem.as_deref(), Some(b"CERT".as_ref()));
        assert_eq!(m.client_key_pem.as_ref().map(|z| z.as_slice()), Some(b"KEY".as_ref()));
    }

    #[test]
    fn inline_overrides_secret_for_public_material() {
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some("INLINE_CA".into()),
                client_cert: Some("INLINE_CERT".into()),
                secret_ref: None,
                secret: Some(secret_with(&[
                    ("ca.crt", b"SECRET_CA"),
                    ("tls.crt", b"SECRET_CERT"),
                    ("tls.key", b"KEY"),
                ])),
            }),
            insecure_skip_verify: None,
        };
        let m = build_material(&tls).unwrap();
        assert_eq!(m.ca_pem.as_deref(), Some(b"INLINE_CA".as_ref()));
        assert_eq!(m.client_cert_pem.as_deref(), Some(b"INLINE_CERT".as_ref()));
        // key always from secret
        assert_eq!(m.client_key_pem.as_ref().map(|z| z.as_slice()), Some(b"KEY".as_ref()));
    }

    #[test]
    fn inline_only_ca() {
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some("CA".into()),
                client_cert: None,
                secret_ref: None,
                secret: None,
            }),
            insecure_skip_verify: None,
        };
        let m = build_material(&tls).unwrap();
        assert_eq!(m.ca_pem.as_deref(), Some(b"CA".as_ref()));
        assert!(m.client_cert_pem.is_none());
        assert!(m.client_key_pem.is_none());
    }

    #[test]
    fn empty_certs_returns_none() {
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: None,
                client_cert: None,
                secret_ref: None,
                secret: None,
            }),
            insecure_skip_verify: None,
        };
        assert!(build_material(&tls).is_none());
    }

    #[test]
    fn combined_identity_pem_concatenates() {
        let m = TlsMaterial {
            ca_pem: None,
            client_cert_pem: Some(b"CERT".to_vec()),
            client_key_pem: Some(Zeroizing::new(b"KEY".to_vec())),
        };
        let combined = combined_identity_pem(&m).unwrap();
        assert_eq!(combined.as_slice(), b"CERT\nKEY");
    }

    #[test]
    fn combined_identity_pem_none_without_key() {
        let m = TlsMaterial {
            ca_pem: None,
            client_cert_pem: Some(b"CERT".to_vec()),
            client_key_pem: None,
        };
        assert!(combined_identity_pem(&m).is_none());
    }

    // ------------------------------------------------------------------
    // build_rustls_client_config
    // ------------------------------------------------------------------

    /// With both `ring` and `aws-lc-rs` features reachable in the dep graph
    /// (fred's `enable-rustls` pulls `tokio-rustls/aws_lc_rs`; we enable
    /// `rustls/ring` directly), rustls cannot auto-pick a default
    /// CryptoProvider. The production binaries install ring explicitly — do
    /// the same in tests so `ClientConfig::builder()` does not panic.
    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    /// Generate a self-signed CA cert, a client leaf cert, and its private
    /// key (all PEM) for ClientConfig construction tests. The CA and leaf
    /// are independent self-signed roots — good enough for exercising PEM
    /// parsing + RootCertStore population; we never do a live handshake.
    fn gen_ca_and_leaf() -> (String, String, String) {
        let ca = rcgen::generate_simple_self_signed(vec!["test-ca".into()]).unwrap();
        let leaf = rcgen::generate_simple_self_signed(vec!["redis.example.com".into()]).unwrap();
        (ca.cert.pem(), leaf.cert.pem(), leaf.signing_key.serialize_pem())
    }

    #[test]
    fn rustls_config_none_without_material() {
        install_test_crypto_provider();
        let tls = TlsConfig {
            enabled: true,
            certs: None,
            insecure_skip_verify: None,
        };
        assert!(build_rustls_client_config(&tls).unwrap().is_none());
    }

    #[test]
    fn rustls_config_with_inline_ca() {
        install_test_crypto_provider();
        let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some(ca_pem),
                client_cert: None,
                secret_ref: None,
                secret: None,
            }),
            insecure_skip_verify: None,
        };
        let cfg = build_rustls_client_config(&tls).unwrap();
        assert!(cfg.is_some(), "expected a ClientConfig when CA is provided");
    }

    #[test]
    fn rustls_config_with_ca_and_client_mtls() {
        install_test_crypto_provider();
        let (ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some(ca_pem.clone()),
                client_cert: Some(cert_pem.clone()),
                secret_ref: None,
                // Private key can only come from Secret per D2; simulate that.
                secret: Some(secret_with(&[("tls.key", key_pem.as_bytes())])),
            }),
            insecure_skip_verify: None,
        };
        let cfg = build_rustls_client_config(&tls)
            .expect("should build ClientConfig")
            .expect("config present");
        // Client auth resolver must have been attached (non-dummy).
        assert!(cfg.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn rustls_config_invalid_ca_pem_errors() {
        install_test_crypto_provider();
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some("-----BEGIN CERTIFICATE-----\nnot-a-real-pem\n-----END CERTIFICATE-----\n".into()),
                client_cert: None,
                secret_ref: None,
                secret: None,
            }),
            insecure_skip_verify: None,
        };
        assert!(build_rustls_client_config(&tls).is_err());
    }

    #[test]
    fn rustls_config_system_roots_when_ca_absent() {
        install_test_crypto_provider();
        // No inline CA, but a client cert pair → must fall back to system
        // roots for the server chain. We don't assert the specific root set
        // (platform-dependent); only that construction succeeds.
        let (_ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: None,
                client_cert: Some(cert_pem),
                secret_ref: None,
                secret: Some(secret_with(&[("tls.key", key_pem.as_bytes())])),
            }),
            insecure_skip_verify: None,
        };
        // CI sandboxes may have an empty system trust store; accept either
        // success (normal dev machines) or the specific "no system CA" error.
        match build_rustls_client_config(&tls) {
            Ok(Some(_)) => {}
            Err(e) => assert!(
                e.to_string().contains("system CA certificates"),
                "unexpected error: {e}"
            ),
            Ok(None) => panic!("expected Some(config) or Err, got Ok(None)"),
        }
    }
}
