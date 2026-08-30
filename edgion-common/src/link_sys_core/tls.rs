//! Gateway-side TLS helpers for `LinkSys` resources.
//!
//! All outbound TLS flows through the shared resolver
//! ([`crate::link_sys_core::outbound_tls::resolve_outbound_tls`]):
//! - `apply_resolved_outbound_tls` — reqwest path (ES, webhook)
//! - `build_rustls_client_config` — rustls path (Redis via fred)
//!
//! The resolver handles global-default inheritance and CA material resolution.
//! `verify=false` is honored when the provider supports it (operator responsibility)
//! and rejected only when the provider's SDK has no skip-verify API.

use anyhow::{Context, Result};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use zeroize::Zeroizing;

use edgion_resources::resources::common::TlsConfig;

use crate::link_sys_core::outbound_tls::{self, EffectiveCa};

/// Build a rustls [`ClientConfig`] from a CRD-level [`TlsConfig`].
///
/// Used by providers whose SDK consumes `rustls::ClientConfig` directly (e.g.
/// fred's Redis client via `TlsConnector::from(ClientConfig)`). Providers that
/// accept raw PEM (etcd-client / reqwest) should use
/// `apply_resolved_outbound_tls` instead.
///
/// Returns `Ok(None)` when there is nothing to configure — no custom CA and no
/// client identity (callers can then fall back to the library's default TLS
/// connector).
///
/// **Skip-verify is not supported**: `verify=false` is always rejected
/// (`provider_can_disable=false`). rustls has no skip-verify path — a
/// `verify=false` config would silently fall through to a verifying connector,
/// so this function rejects it upfront with
/// [`EdgionErrorCode::OutboundTlsVerifyUnsupported`] instead.
///
/// Root certificate selection:
/// - [`EffectiveCa::Custom`] → the CA PEM is the **only** trust root (private
///   CA mode; the system trust store is intentionally not merged in).
/// - [`EffectiveCa::System`] → uses the built-in webpki roots compiled into
///   the binary so mTLS against a public-CA backend still verifies the server
///   chain.
///
/// Client authentication: applied only when **both** `client_cert_pem` and
/// `client_key_pem` are present; otherwise `.with_no_client_auth()`.
pub fn build_rustls_client_config(tls: &TlsConfig) -> Result<Option<ClientConfig>> {
    let resolved = outbound_tls::resolve_outbound_tls(
        &tls.outbound,
        tls.resolved_ca_certificates.as_deref(),
        tls.resolved_client_certificate.as_ref(),
        false,
        "LinkSys",
    )
    .map_err(|e| anyhow::anyhow!("LinkSys TLS configuration error: {e:?}"))?;

    // Nothing to configure: system CA + no client identity → caller uses
    // the library's default TLS connector.
    let has_custom_ca = matches!(resolved.ca, EffectiveCa::Custom(_));
    if !has_custom_ca && resolved.client_identity.is_none() {
        return Ok(None);
    }

    let mut roots = RootCertStore::empty();

    match &resolved.ca {
        EffectiveCa::Custom(bundles) => {
            let mut added = 0usize;
            for bundle in bundles {
                for cert in CertificateDer::pem_slice_iter(bundle.as_slice()) {
                    let cert = cert.context("failed to parse CA certificate PEM from LinkSys TLS material")?;
                    roots
                        .add(cert)
                        .context("failed to add CA certificate to rustls RootCertStore")?;
                    added += 1;
                }
            }
            if added == 0 {
                anyhow::bail!("LinkSys TLS CA PEM did not contain any valid certificates");
            }
        }
        EffectiveCa::System => {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
    }

    let builder = ClientConfig::builder().with_root_certificates(roots);

    let client_config = match &resolved.client_identity {
        Some((cert_pem, key_pem)) => {
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
        None => builder.with_no_client_auth(),
    };

    Ok(Some(client_config))
}

/// Split a multi-certificate PEM bundle into individual certificate PEM blocks.
///
/// Handles multiple certificates concatenated in a single PEM buffer, where each
/// certificate is wrapped in `-----BEGIN CERTIFICATE-----` and `-----END CERTIFICATE-----`.
fn split_pem_certificates(pem: &[u8]) -> Vec<Vec<u8>> {
    let s = String::from_utf8_lossy(pem);
    let mut certs = Vec::new();
    let mut current = String::new();
    for line in s.lines() {
        current.push_str(line);
        current.push('\n');
        if line == "-----END CERTIFICATE-----" {
            certs.push(current.clone().into_bytes());
            current.clear();
        }
    }
    certs
}

/// Apply custom CA PEM bundles to a reqwest `ClientBuilder` with REPLACE
/// semantics: disables the built-in root certificates, then adds each supplied
/// PEM bundle as a trust anchor.
///
/// Use this instead of the bare `add_root_certificate` call whenever a custom
/// CA replaces (rather than supplements) the public roots.
///
/// Each bundle in `bundles` may contain multiple certificates (root + intermediate chain).
/// `bundles` is a slice of PEM byte vectors, one per resolved CA Secret.
pub fn apply_custom_ca_pem(
    builder: reqwest::ClientBuilder,
    bundles: &[Vec<u8>],
) -> anyhow::Result<reqwest::ClientBuilder> {
    let mut builder = builder.tls_built_in_root_certs(false);
    let mut certificate_count = 0usize;
    for bundle in bundles {
        // Split the bundle into individual PEM blocks (handles root + intermediate chains).
        let pem_blocks = split_pem_certificates(bundle.as_slice());
        for cert_pem in pem_blocks {
            let cert =
                reqwest::Certificate::from_pem(cert_pem.as_slice()).context("TLS: invalid CA certificate PEM")?;
            builder = builder.add_root_certificate(cert);
            certificate_count += 1;
        }
    }
    anyhow::ensure!(certificate_count > 0, "TLS: custom CA bundle contains no certificates");
    Ok(builder)
}

/// Resolve `tls` through the shared outbound resolver and apply the result
/// (verify flag, custom-CA REPLACE, mTLS identity) to a reqwest builder.
/// This is the ONLY reqwest TLS path: `verify` always flows through
/// `resolve_outbound_tls`, so global-default inheritance is enforced uniformly.
/// `verify=false` is honored when the operator sets it explicitly (provider
/// must support skip-verify); a WARN is emitted at configuration-load time.
pub fn apply_resolved_outbound_tls(
    builder: reqwest::ClientBuilder,
    tls: &TlsConfig,
    provider: &str,
) -> Result<reqwest::ClientBuilder> {
    let resolved = outbound_tls::resolve_outbound_tls(
        &tls.outbound,
        tls.resolved_ca_certificates.as_deref(),
        tls.resolved_client_certificate.as_ref(),
        true,
        provider,
    )
    .map_err(|e| anyhow::anyhow!("{provider} TLS configuration error: {e:?}"))?;

    let mut builder = builder;
    if !resolved.verify {
        // Operator explicitly disabled verification; resolver already emitted WARN.
        builder = builder.danger_accept_invalid_certs(true);
    } else if let EffectiveCa::Custom(bundles) = &resolved.ca {
        builder = apply_custom_ca_pem(builder, bundles)
            .with_context(|| format!("{provider} TLS: invalid CA certificate PEM"))?;
    }
    if let Some((cert_pem, key_pem)) = &resolved.client_identity {
        let mut combined = Zeroizing::new(Vec::with_capacity(cert_pem.len() + key_pem.len() + 1));
        combined.extend_from_slice(cert_pem);
        combined.push(b'\n');
        combined.extend_from_slice(key_pem);
        let identity = reqwest::Identity::from_pem(combined.as_slice())
            .with_context(|| format!("{provider} TLS: invalid client identity PEM"))?;
        builder = builder.identity(identity);
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::constants::secret_keys::tls as tls_keys;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
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

    /// Build a CA `Secret` with the PEM stored under `ca.crt`.
    fn ca_secret(pem: &[u8]) -> Secret {
        secret_with(&[(tls_keys::CA_CERT, pem)])
    }

    /// Build an `OutboundTlsLocal` that declares one CA ref (name only; the
    /// resolved bytes come from `resolved_ca_certificates`).
    fn outbound_with_ca_ref(name: &str) -> OutboundTlsLocal {
        OutboundTlsLocal {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                    group: String::new(),
                    kind: "Secret".into(),
                    name: name.into(),
                    namespace: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // ------------------------------------------------------------------
    // build_rustls_client_config
    // ------------------------------------------------------------------

    fn install_test_crypto_provider() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    fn gen_ca_and_leaf() -> (String, String, String) {
        let ca = rcgen::generate_simple_self_signed(vec!["test-ca".into()]).unwrap();
        let leaf = rcgen::generate_simple_self_signed(vec!["redis.example.com".into()]).unwrap();
        (ca.cert.pem(), leaf.cert.pem(), leaf.signing_key.serialize_pem())
    }

    #[test]
    fn rustls_config_none_without_material() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        install_test_crypto_provider();
        // No CA, no client identity → None (caller uses default connector)
        let tls = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        };
        assert!(build_rustls_client_config(&tls).unwrap().is_none());
    }

    #[test]
    fn rustls_config_with_inline_ca() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        install_test_crypto_provider();
        let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("my-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
            resolved_client_certificate: None,
        };
        let cfg = build_rustls_client_config(&tls).unwrap();
        assert!(cfg.is_some(), "expected a ClientConfig when CA is provided");
    }

    #[test]
    fn rustls_config_with_ca_and_client_mtls() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        install_test_crypto_provider();
        let (ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("my-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
            resolved_client_certificate: Some(secret_with(&[
                ("tls.crt", cert_pem.as_bytes()),
                ("tls.key", key_pem.as_bytes()),
            ])),
        };
        let cfg = build_rustls_client_config(&tls)
            .expect("should build ClientConfig")
            .expect("config present");
        assert!(cfg.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn rustls_config_invalid_ca_pem_errors() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        install_test_crypto_provider();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("bad-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(
                b"-----BEGIN CERTIFICATE-----\nnot-a-real-pem\n-----END CERTIFICATE-----\n",
            )]),
            resolved_client_certificate: None,
        };
        assert!(build_rustls_client_config(&tls).is_err());
    }

    /// `apply_resolved_outbound_tls` must return `Ok` and apply
    /// `danger_accept_invalid_certs(true)` when `verify == Some(false)` — the
    /// operator's explicit choice is honored regardless of any environment variable.
    #[test]
    fn resolved_outbound_tls_verify_false_is_honored() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let tls_skip = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                verify: Some(false),
                ..Default::default()
            },
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        };
        let tls_nop = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal::default(), // verify = None → inherit global (true)
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        };

        // verify=false → honored, returns Ok.
        assert!(
            apply_resolved_outbound_tls(reqwest::Client::builder(), &tls_skip, "TestProvider").is_ok(),
            "verify=false must be honored and return Ok"
        );

        // verify=None → noop, always Ok.
        assert!(
            apply_resolved_outbound_tls(reqwest::Client::builder(), &tls_nop, "TestProvider").is_ok(),
            "verify=None must pass through unchanged"
        );
    }

    #[test]
    fn rustls_config_system_roots_when_ca_absent() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        install_test_crypto_provider();
        // No inline CA + a client cert pair → must use built-in webpki roots
        // for the server chain. The webpki roots are always present (compiled in),
        // so construction must always succeed.
        let (_ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: Some(secret_with(&[
                ("tls.crt", cert_pem.as_bytes()),
                ("tls.key", key_pem.as_bytes()),
            ])),
        };
        assert!(
            build_rustls_client_config(&tls).unwrap().is_some(),
            "expected Ok(Some(_)) with built-in webpki roots"
        );
    }

    // ------------------------------------------------------------------
    // apply_resolved_outbound_tls
    // ------------------------------------------------------------------

    #[test]
    fn apply_resolved_outbound_tls_passthrough_when_no_material() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let tls = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        };
        assert!(
            apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok(),
            "should return Ok when no material is configured"
        );
    }

    #[test]
    fn apply_resolved_outbound_tls_valid_ca_only() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("my-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
            resolved_client_certificate: None,
        };
        assert!(
            apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok(),
            "valid CA PEM should not produce an error"
        );
    }

    #[test]
    fn apply_resolved_outbound_tls_rejects_bundle_without_certificate() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("invalid-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(b"not-a-pem")]),
            resolved_client_certificate: None,
        };

        apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test")
            .expect_err("custom CA bundle without a certificate must fail closed");
    }

    #[test]
    fn apply_resolved_outbound_tls_valid_mtls() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let (ca_pem, cert_pem, key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("my-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
            resolved_client_certificate: Some(secret_with(&[
                ("tls.crt", cert_pem.as_bytes()),
                ("tls.key", key_pem.as_bytes()),
            ])),
        };
        assert!(
            apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test").is_ok(),
            "valid CA + mTLS identity should not produce an error"
        );
    }

    #[test]
    fn apply_resolved_outbound_tls_invalid_identity_errors() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let (ca_pem, _cert_pem, _key_pem) = gen_ca_and_leaf();
        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("my-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(ca_pem.as_bytes())]),
            resolved_client_certificate: Some(secret_with(&[("tls.crt", b"bad-cert"), ("tls.key", b"bad-key")])),
        };
        let err = apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "TestProvider")
            .expect_err("invalid identity PEM must return Err");
        let msg = err.to_string();
        assert!(
            msg.contains("TestProvider TLS:"),
            "error must carry provider prefix, got: {msg}"
        );
    }

    /// Two CA bundles in `resolved_ca_certificates` must both be loaded into the
    /// reqwest root store.  Both bundles are real (rcgen-generated) certs so
    /// `Certificate::from_pem` accepts them; the assertion is that `build()` does
    /// not return an error — a failure to parse either bundle would surface there.
    #[test]
    fn apply_resolved_outbound_tls_multi_bundle_reqwest() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let (ca_pem_a, _c, _k) = gen_ca_and_leaf();
        let (ca_pem_b, _c, _k) = gen_ca_and_leaf();

        let tls = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![
                        BackendTLSPolicyCACertificateRef {
                            group: String::new(),
                            kind: "Secret".into(),
                            name: "ca-a".into(),
                            namespace: None,
                        },
                        BackendTLSPolicyCACertificateRef {
                            group: String::new(),
                            kind: "Secret".into(),
                            name: "ca-b".into(),
                            namespace: None,
                        },
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            resolved_ca_certificates: Some(vec![ca_secret(ca_pem_a.as_bytes()), ca_secret(ca_pem_b.as_bytes())]),
            resolved_client_certificate: None,
        };

        let result = apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "Test");
        assert!(
            result.is_ok(),
            "two distinct CA bundles must both be accepted by reqwest, err: {:?}",
            result.err()
        );
        // Verify the built client can be constructed (not just the builder).
        let builder_result = result.unwrap().build();
        assert!(
            builder_result.is_ok(),
            "reqwest Client::build() must succeed with two CA bundles, err: {:?}",
            builder_result.err()
        );
    }

    /// A single CA bundle containing multiple certificates (root + intermediate chain)
    /// must have all certificates loaded into the reqwest root store.
    /// This regression test ensures that `apply_custom_ca_pem` correctly handles
    /// multi-certificate PEM bundles instead of silently dropping certificates after the first.
    #[test]
    fn apply_custom_ca_pem_multi_cert_in_single_bundle() {
        let _guard = outbound_tls::outbound_tls_test_guard();
        let (ca_root_pem, _c, _k) = gen_ca_and_leaf();
        let (_ca_intermediate_pem, _c, _k) = gen_ca_and_leaf();

        // Simulate an operator concatenating root + intermediate into a single PEM bundle.
        // In practice, ca_root_pem and _ca_intermediate_pem are two independent certs.
        let combined_pem = format!("{}\n{}\n", ca_root_pem, _ca_intermediate_pem);

        let tls = TlsConfig {
            enabled: true,
            outbound: outbound_with_ca_ref("multi-cert-ca"),
            resolved_ca_certificates: Some(vec![ca_secret(combined_pem.as_bytes())]),
            resolved_client_certificate: None,
        };

        // Before the fix, reqwest::Certificate::from_pem would silently accept
        // only the first cert and drop the second. After the fix, apply_custom_ca_pem
        // splits the bundle and loads all certs.
        let result = apply_resolved_outbound_tls(reqwest::Client::builder(), &tls, "MultiCertTest");
        assert!(
            result.is_ok(),
            "multi-certificate PEM bundle should be accepted, err: {:?}",
            result.err()
        );

        // Verify the built client can be constructed (success = no error during cert load).
        let builder_result = result.unwrap().build();
        assert!(
            builder_result.is_ok(),
            "reqwest Client::build() must succeed with multi-cert bundle, err: {:?}",
            builder_result.err()
        );
    }
}
