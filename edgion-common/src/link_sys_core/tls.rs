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

    resolved.reject_unsupported_identity_policy("LinkSys")?;

    // Nothing to configure: system CA + no client identity → caller uses
    // the library's default TLS connector.
    let has_custom_ca = matches!(resolved.ca, EffectiveCa::Custom(_));
    if !has_custom_ca && resolved.client_identity.is_none() {
        return Ok(None);
    }

    let mut roots = RootCertStore::empty();

    match &resolved.ca {
        EffectiveCa::Custom(bundles) => {
            if bundles.is_empty() {
                anyhow::bail!("LinkSys TLS CA PEM did not contain any valid certificates");
            }
            for bundle in bundles {
                let mut added = 0usize;
                for cert in CertificateDer::pem_slice_iter(bundle.as_slice()) {
                    let cert = cert.context("failed to parse CA certificate PEM from LinkSys TLS material")?;
                    roots
                        .add(cert)
                        .context("failed to add CA certificate to rustls RootCertStore")?;
                    added += 1;
                }
                if added == 0 {
                    anyhow::bail!("LinkSys TLS CA PEM bundle did not contain any valid certificates");
                }
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
    anyhow::ensure!(!bundles.is_empty(), "TLS: custom CA bundle contains no certificates");
    for bundle in bundles {
        let parsed = crate::tls_validation::parse_canonical_ca_bundle(bundle)
            .map_err(|_| anyhow::anyhow!("TLS: invalid CA certificate PEM bundle"))?;
        for der in parsed.certificates_der {
            let cert = reqwest::Certificate::from_der(&der).context("TLS: invalid CA certificate DER")?;
            builder = builder.add_root_certificate(cert);
        }
    }
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

    resolved.reject_unsupported_identity_policy(provider)?;

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
#[path = "../../tests/unit/link_sys_core/tls_test.rs"]
mod tests;
