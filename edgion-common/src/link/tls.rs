//! Gateway-side TLS material resolution for `LinkSys` resources.
//!
//! Each provider (Redis / Etcd / Elasticsearch / Webhook) used to build its own
//! TLS config from the per-resource `*Tls` struct. After R1 these are all
//! unified behind [`edgion_resources::resources::common::TlsConfig`]; this module
//! centralizes the lookup / merge logic so every provider consumes the same
//! [`TlsMaterial`] shape. It also houses the reqwest-specific helpers
//! ([`apply_insecure_skip_verify`], [`apply_to_reqwest_builder`]) so all
//! reqwest-based providers (ES, webhook) share one wiring path.
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

use edgion_resources::credentials::{SecretMaterialResolver, TlsCertsResolver, TlsMaterial};
use edgion_resources::resources::common::TlsConfig;

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

/// Apply CA root and mTLS identity from a [`TlsConfig`] to a reqwest client builder.
///
/// Factored out of the ES and webhook providers so all reqwest-based providers
/// share the same wiring and error messages. Call this **after**
/// [`apply_insecure_skip_verify`]. Whether to guard on `tls.enabled` is the
/// caller's responsibility — ES gates this inside `if tls.enabled { … }`;
/// webhook enters the TLS branch whenever `config.tls` is `Some(_)`.
///
/// `provider` is a short display name (e.g. `"Elasticsearch"`, `"Webhook"`)
/// embedded in error messages so operators can identify which provider produced
/// the invalid PEM.
///
/// Returns the builder unchanged when [`build_material`] yields `None` (no
/// certs configured).
pub fn apply_to_reqwest_builder(
    builder: reqwest::ClientBuilder,
    tls: &TlsConfig,
    provider: &str,
) -> Result<reqwest::ClientBuilder> {
    let Some(material) = build_material(tls) else {
        return Ok(builder);
    };
    let mut builder = builder;
    if let Some(ca) = &material.ca_pem {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(ca)
                .with_context(|| format!("{provider} TLS: invalid CA certificate PEM"))?,
        );
    }
    if let Some(combined) = combined_identity_pem(&material) {
        builder = builder.identity(
            reqwest::Identity::from_pem(combined.as_slice())
                .with_context(|| format!("{provider} TLS: invalid client identity PEM"))?,
        );
    }
    Ok(builder)
}

/// Cross-test mutex for any test that reads/writes `EDGION_ENV`. `cargo test`
/// runs tests within a single binary in parallel by default, and `std::env::set_var`
/// mutates a process-global table; concurrent env-touching tests can clobber each
/// other's `set_var`/`remove_var` calls and produce flaky CI failures. Each such
/// test must `let _guard = ENV_TEST_MUTEX.lock();` for the duration of its
/// snapshot/mutate/restore window. A shared mutex is used (not one per file) so
/// tests in `tls.rs` and `providers/elasticsearch/config_mapping.rs` serialize
/// against each other.
#[cfg(any(test, feature = "test-util"))]
pub static ENV_TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Apply the `tls.insecure_skip_verify` flag to a reqwest client builder under a
/// uniform cross-provider policy.
///
/// Policy when `insecure_skip_verify == Some(true)`:
/// - `EDGION_ENV=production` → return an error so the LinkSys refuses to start,
///   surfacing the misconfiguration loudly instead of silently disabling
///   verification on a production data path.
/// - otherwise → emit a warning and call `danger_accept_invalid_certs(true)`.
///
/// When `insecure_skip_verify` is unset or `false`, the builder is returned
/// unchanged.
///
/// Caller responsibilities:
/// - When `tls.enabled == false`, skip this entirely (the flag is moot when TLS
///   is not enabled). Mirror the ES behaviour by only calling this helper from
///   inside an `if tls.enabled { ... }` branch.
/// - Redis / Etcd SDKs do not expose a skip-verify knob — those providers
///   `bail!` at the call site rather than going through this helper.
pub fn apply_insecure_skip_verify(
    builder: reqwest::ClientBuilder,
    tls: &TlsConfig,
    provider: &str,
) -> Result<reqwest::ClientBuilder> {
    if tls.insecure_skip_verify != Some(true) {
        return Ok(builder);
    }
    let is_prod = std::env::var("EDGION_ENV")
        .map(|v| v.eq_ignore_ascii_case("production"))
        .unwrap_or(false);
    if is_prod {
        anyhow::bail!(
            "{} TLS insecure_skip_verify cannot be enabled in production (EDGION_ENV=production). \
             Use a proper CA certificate or disable TLS verification only in non-production environments.",
            provider
        );
    }
    // TODO(g9a-003-followup): once `edgion_system_error_state` (skip-list #12) is
    // wired into the gateway crate, increment a labelled counter / set a gauge
    // here so an operator can alert on "TLS verification disabled" outside of
    // log scrapers. The deferral is intentional — the gauge is not yet defined
    // anywhere in the codebase and wiring it would explode the safety-fix scope.
    tracing::warn!(
        provider = provider,
        "TLS certificate verification disabled — insecure! Not allowed when EDGION_ENV=production",
    );
    Ok(builder.danger_accept_invalid_certs(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::{TlsCerts, TlsConfig};
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

    /// Table-driven tests for `build_material`.
    ///
    /// `expected` encodes the return value:
    ///   - `None`  → `build_material` must return `None`
    ///   - `Some((ca, cert, key))` → must return `Some(material)` with those field values
    ///
    /// The "key always from secret, never from inline" invariant is explicit in case
    /// `inline_overrides_secret_for_public_material`: inline ca/cert win, but the key
    /// is still the one from the secret, not overridable by any inline field.
    #[test]
    fn test_build_material_matrix() {
        struct BuildMaterialCase {
            name: &'static str,
            tls: TlsConfig,
            /// None  → expect build_material to return None
            /// Some((ca, cert, key)) → expect Some(material) with these field values
            expected: Option<(Option<&'static [u8]>, Option<&'static [u8]>, Option<&'static [u8]>)>,
        }

        let cases = vec![
            // Case 1 (was: returns_none_without_certs)
            // certs field absent → None
            BuildMaterialCase {
                name: "returns_none_without_certs",
                tls: TlsConfig {
                    enabled: true,
                    certs: None,
                    insecure_skip_verify: None,
                },
                expected: None,
            },
            // Case 2 (was: extracts_from_secret_only)
            // All three fields sourced from secret; no inline overrides present
            BuildMaterialCase {
                name: "extracts_from_secret_only",
                tls: TlsConfig {
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
                },
                expected: Some((Some(b"CA"), Some(b"CERT"), Some(b"KEY"))),
            },
            // Case 3 (was: inline_overrides_secret_for_public_material)
            // Inline ca/cert override their secret counterparts; the key is NOT
            // overridable by inline — it always comes from the secret.
            BuildMaterialCase {
                name: "inline_overrides_secret_for_public_material",
                tls: TlsConfig {
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
                },
                // ca and cert come from inline; key still from secret (invariant)
                expected: Some((Some(b"INLINE_CA"), Some(b"INLINE_CERT"), Some(b"KEY"))),
            },
            // Case 4 (was: inline_only_ca)
            // Only inline ca provided; no secret → cert and key are absent
            BuildMaterialCase {
                name: "inline_only_ca",
                tls: TlsConfig {
                    enabled: true,
                    certs: Some(TlsCerts {
                        ca_cert: Some("CA".into()),
                        client_cert: None,
                        secret_ref: None,
                        secret: None,
                    }),
                    insecure_skip_verify: None,
                },
                expected: Some((Some(b"CA"), None, None)),
            },
            // Case 5 (was: empty_certs_returns_none)
            // TlsCerts present but all fields None → None
            BuildMaterialCase {
                name: "empty_certs_returns_none",
                tls: TlsConfig {
                    enabled: true,
                    certs: Some(TlsCerts {
                        ca_cert: None,
                        client_cert: None,
                        secret_ref: None,
                        secret: None,
                    }),
                    insecure_skip_verify: None,
                },
                expected: None,
            },
        ];

        for case in &cases {
            match &case.expected {
                None => {
                    assert!(
                        build_material(&case.tls).is_none(),
                        "[{}] expected None but got Some",
                        case.name,
                    );
                }
                Some((exp_ca, exp_cert, exp_key)) => {
                    let m = build_material(&case.tls)
                        .unwrap_or_else(|| panic!("[{}] expected Some but got None", case.name));
                    assert_eq!(m.ca_pem.as_deref(), *exp_ca, "[{}] ca_pem mismatch", case.name,);
                    assert_eq!(
                        m.client_cert_pem.as_deref(),
                        *exp_cert,
                        "[{}] client_cert_pem mismatch",
                        case.name,
                    );
                    assert_eq!(
                        m.client_key_pem.as_ref().map(|z| z.as_slice()),
                        *exp_key,
                        "[{}] client_key_pem mismatch — key must always come from secret, never from inline",
                        case.name,
                    );
                }
            }
        }
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

    /// `apply_insecure_skip_verify` must `bail!` when `EDGION_ENV=production` and the
    /// flag is set, so a misconfigured LinkSys refuses to start instead of silently
    /// disabling certificate verification on a production data path.
    ///
    /// The non-prod branch must accept the flag (warn + apply) so dev/staging keeps
    /// the existing escape hatch.
    ///
    /// Note: this test mutates the global `EDGION_ENV` env var. The
    /// `super::ENV_TEST_MUTEX` guard serialises against any other env-touching
    /// test (e.g. ES `test_tls_disabled_ignores_insecure_skip_verify_even_in_prod`)
    /// so concurrent runs in the same test binary cannot clobber each other's
    /// `set_var` / `remove_var` calls. The prior value is snapshotted and
    /// restored before the guard releases.
    #[test]
    fn apply_insecure_skip_verify_prod_bails_nonprod_accepts() {
        let _env_guard = super::ENV_TEST_MUTEX.lock();

        let tls_skip = TlsConfig {
            enabled: true,
            certs: None,
            insecure_skip_verify: Some(true),
        };
        let tls_nop = TlsConfig {
            enabled: true,
            certs: None,
            insecure_skip_verify: None,
        };

        let prior = std::env::var("EDGION_ENV").ok();

        // 1. Production → bail.
        std::env::set_var("EDGION_ENV", "production");
        let prod_err = apply_insecure_skip_verify(reqwest::Client::builder(), &tls_skip, "TestProvider").err();
        assert!(
            prod_err
                .as_ref()
                .is_some_and(|e| e.to_string().contains("EDGION_ENV=production")),
            "prod must bail with EDGION_ENV mention, got: {:?}",
            prod_err
        );

        // 2. Non-production (or unset) → accepts. We don't assert on the resulting
        //    builder's internal state (reqwest exposes no introspection); the contract
        //    is that no error is returned.
        std::env::remove_var("EDGION_ENV");
        assert!(
            apply_insecure_skip_verify(reqwest::Client::builder(), &tls_skip, "TestProvider").is_ok(),
            "non-prod must accept insecure_skip_verify=true"
        );

        // 3. Flag absent → noop pass-through, even in production.
        std::env::set_var("EDGION_ENV", "production");
        assert!(
            apply_insecure_skip_verify(reqwest::Client::builder(), &tls_nop, "TestProvider").is_ok(),
            "missing insecure_skip_verify must pass through unchanged"
        );

        // Restore prior state.
        match prior {
            Some(v) => std::env::set_var("EDGION_ENV", v),
            None => std::env::remove_var("EDGION_ENV"),
        }
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

    // ------------------------------------------------------------------
    // apply_to_reqwest_builder
    // ------------------------------------------------------------------

    #[test]
    fn apply_to_reqwest_builder_passthrough_when_no_material() {
        let tls = TlsConfig {
            enabled: true,
            certs: None,
            insecure_skip_verify: None,
        };
        assert!(
            apply_to_reqwest_builder(reqwest::Client::builder(), &tls, "Test").is_ok(),
            "should return Ok when build_material yields None"
        );
    }

    #[test]
    fn apply_to_reqwest_builder_valid_ca_only() {
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
        assert!(
            apply_to_reqwest_builder(reqwest::Client::builder(), &tls, "Test").is_ok(),
            "valid CA PEM should not produce an error"
        );
    }

    #[test]
    fn apply_to_reqwest_builder_valid_mtls() {
        let (ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some(ca_pem),
                client_cert: Some(cert_pem),
                secret_ref: None,
                secret: Some(secret_with(&[("tls.key", key_pem.as_bytes())])),
            }),
            insecure_skip_verify: None,
        };
        assert!(
            apply_to_reqwest_builder(reqwest::Client::builder(), &tls, "Test").is_ok(),
            "valid CA + mTLS identity should not produce an error"
        );
    }

    #[test]
    fn apply_to_reqwest_builder_invalid_identity_errors() {
        let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            certs: Some(TlsCerts {
                ca_cert: Some(ca_pem),
                client_cert: Some("bad-cert".into()),
                secret_ref: None,
                secret: Some(secret_with(&[("tls.key", b"bad-key")])),
            }),
            insecure_skip_verify: None,
        };
        let err = apply_to_reqwest_builder(reqwest::Client::builder(), &tls, "TestProvider")
            .expect_err("invalid identity PEM must return Err");
        let msg = err.to_string();
        assert!(
            msg.contains("TestProvider TLS:"),
            "error must carry provider prefix, got: {msg}"
        );
    }
}
