//! Canonical TLS conversion helpers for `LinkSys` resources.
//!
//! All outbound TLS flows through the shared resolver
//! ([`crate::link_sys_core::outbound_tls`]):
//! - `apply_resolved_outbound_tls` — reqwest path (ES, webhook)
//! - `build_validated_rustls_client_config` — actual-owner Redis/Fred path
//! - `build_kafka_rustls_client_config` — actual-owner Kafka/rskafka path
//!
//! The resolver handles global-default inheritance and CA material resolution.
//! `verify=false` is honored when the provider supports it (operator responsibility)
//! and rejected only when the provider's SDK has no skip-verify API.

use anyhow::{Context, Result};
use rustls::pki_types::{
    pem::{PemObject, SectionKind},
    CertificateDer, PrivateKeyDer,
};
use rustls::{ClientConfig, RootCertStore};
use zeroize::Zeroizing;

use edgion_resources::resources::common::TlsConfig;

use crate::link_sys_core::outbound_tls::{self, EffectiveCa};

/// Build from an actual owning selection and one captured inherited generation.
///
/// Returns `Ok(None)` when there is nothing to configure — no custom CA and no
/// client identity (callers can then fall back to the library's default TLS
/// connector).
///
/// **Skip-verify is not supported**: `verify=false` is always rejected
/// (`provider_can_disable=false`). The supported connector has no skip-verify path — a
/// `verify=false` config would silently fall through to a verifying connector,
/// so canonical resolution rejects it upfront with
/// `OutboundTlsVerifyUnsupported` instead.
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
pub(crate) fn build_validated_rustls_client_config(
    proof: &outbound_tls::ValidatedLocalTlsSelection<'_>,
    defaults: &outbound_tls::GlobalOutboundTlsSnapshot,
    provider: &str,
) -> Result<Option<ClientConfig>> {
    let resolved = outbound_tls::resolve_validated_outbound_tls_audited(proof, false, provider, defaults)
        .map_err(|error| anyhow::anyhow!("{provider} TLS configuration error: {error:?}"))?;
    build_effective_rustls_client_config(&resolved)
}

fn build_effective_rustls_client_config(resolved: &outbound_tls::ResolvedOutboundTls) -> Result<Option<ClientConfig>> {
    resolved.reject_unsupported_identity_policy("LinkSys")?;

    // Nothing to configure: system CA + no client identity → caller uses
    // the library's default TLS connector.
    let has_custom_ca = matches!(resolved.ca, EffectiveCa::Custom(_));
    if !has_custom_ca && resolved.client_identity.is_none() {
        return Ok(None);
    }

    build_concrete_rustls_client_config(resolved).map(Some)
}

/// Fixed actual Kafka entry. rskafka treats an absent connector as plaintext,
/// so every enabled TLS selection produces a concrete verifying connector.
pub fn build_kafka_rustls_client_config(input: &outbound_tls::PreparedLinkSysTls) -> Result<Option<ClientConfig>> {
    anyhow::ensure!(
        matches!(
            &input.owner().spec.config,
            edgion_resources::resources::link_sys::SystemConfig::Kafka(_)
        ),
        "Kafka TLS construction requires an actual Kafka LinkSys"
    );
    input.with_tls_selection(|proof, defaults| {
        if proof.active_field().is_none() {
            return Ok(None);
        }
        let resolved = outbound_tls::resolve_validated_outbound_tls_audited(proof, false, "Kafka", defaults)
            .map_err(|error| anyhow::anyhow!("Kafka TLS configuration error: {error:?}"))?;
        build_concrete_rustls_client_config(&resolved).map(Some)
    })
}

fn build_concrete_rustls_client_config(resolved: &outbound_tls::ResolvedOutboundTls) -> Result<ClientConfig> {
    resolved.reject_unsupported_identity_policy("LinkSys")?;
    let builder = ClientConfig::builder().with_root_certificates(build_root_store(&resolved.ca)?);

    let identity = match &resolved.client_identity {
        Some((cert_pem, key_pem)) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
                .collect::<std::result::Result<_, _>>()
                .context("failed to parse client certificate PEM from LinkSys TLS material")?;
            if certs.is_empty() {
                anyhow::bail!("LinkSys TLS client certificate PEM did not contain any certificates");
            }
            let key = PrivateKeyDer::from_pem_slice(key_pem.as_slice())
                .context("failed to parse client private key PEM from LinkSys TLS Secret")?;
            Some((certs, key))
        }
        None => None,
    };
    complete_client_auth(builder, identity)
}

fn build_root_store(ca: &EffectiveCa) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();

    match ca {
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

    Ok(roots)
}

type ClientIdentity = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

fn complete_client_auth(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>,
    identity: Option<ClientIdentity>,
) -> Result<ClientConfig> {
    match identity {
        Some((certs, key)) => builder
            .with_client_auth_cert(certs, key)
            .context("failed to configure rustls client authentication"),
        None => Ok(builder.with_no_client_auth()),
    }
}

/// Preserve the former HTTP identity parser: chain order, supported section
/// kinds and last private key win. The combined secret buffer is zeroized.
fn http_client_identity(cert_pem: &[u8], key_pem: &[u8]) -> Result<ClientIdentity> {
    let mut combined = Zeroizing::new(Vec::with_capacity(cert_pem.len() + key_pem.len() + 1));
    combined.extend_from_slice(cert_pem);
    combined.push(b'\n');
    combined.extend_from_slice(key_pem);
    let mut reader = std::io::Cursor::new(combined.as_slice());
    let mut certs = Vec::new();
    let mut key = None;
    while let Some((kind, data)) = rustls::pki_types::pem::from_buf(&mut reader)
        .map_err(|_| anyhow::anyhow!("TLS: invalid client identity PEM"))?
    {
        match kind {
            SectionKind::Certificate => certs.push(data.into()),
            SectionKind::PrivateKey => key = Some(PrivateKeyDer::Pkcs8(data.into())),
            SectionKind::RsaPrivateKey => key = Some(PrivateKeyDer::Pkcs1(data.into())),
            SectionKind::EcPrivateKey => key = Some(PrivateKeyDer::Sec1(data.into())),
            _ => anyhow::bail!("TLS: unsupported client identity PEM section"),
        }
    }
    anyhow::ensure!(!certs.is_empty(), "TLS: client identity contains no certificates");
    Ok((
        certs,
        key.ok_or_else(|| anyhow::anyhow!("TLS: client identity contains no private key"))?,
    ))
}

fn build_http_rustls_client_config(resolved: &outbound_tls::ResolvedOutboundTls) -> Result<ClientConfig> {
    anyhow::ensure!(
        resolved.verify,
        "TLS: verified HTTP configuration requires verification"
    );
    resolved.reject_unsupported_identity_policy("OutboundClient")?;
    if let EffectiveCa::Custom(bundles) = &resolved.ca {
        anyhow::ensure!(!bundles.is_empty(), "TLS: custom CA bundle contains no certificates");
        // Keep HTTP's canonical whole-bundle/DER/cap checks before the shared
        // rustls conversion. This bounded cold-path reparse preserves other
        // providers' existing material-parser contracts.
        for bundle in bundles {
            crate::tls_validation::parse_canonical_ca_bundle(bundle)
                .map_err(|_| anyhow::anyhow!("TLS: invalid CA certificate PEM bundle"))?;
        }
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS: unsupported HTTP protocol configuration")?
        .with_root_certificates(build_root_store(&resolved.ca)?);
    let identity = resolved
        .client_identity
        .as_ref()
        .map(|(cert, key)| http_client_identity(cert, key.as_slice()))
        .transpose()?;
    let mut config = complete_client_auth(builder, identity)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// The absent/inactive HTTP policy uses the same public roots as reqwest 0.12,
/// without reading process-global outbound policy or installing a provider.
pub(crate) fn apply_default_http_tls(builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder> {
    let config = build_http_rustls_client_config(&outbound_tls::ResolvedOutboundTls {
        verify: true,
        ca: EffectiveCa::System,
        client_identity: None,
        san: None,
    })?;
    Ok(builder.tls_backend_preconfigured(config))
}

/// Resolve `tls` through the shared outbound resolver and apply the result
/// (verify flag, custom-CA REPLACE, mTLS identity) to a reqwest builder.
/// Active local policy flows through the canonical snapshot resolver. The HTTP
/// factory separately owns absent/inactive public-root defaults and complete
/// caller-prepared verifier configurations.
/// `verify=false` is honored when the operator sets it explicitly (provider
/// must support skip-verify); a WARN is emitted at configuration-load time.
pub fn apply_resolved_outbound_tls(
    builder: reqwest::ClientBuilder,
    tls: &TlsConfig,
    provider: &str,
) -> Result<reqwest::ClientBuilder> {
    apply_resolved_outbound_tls_with_defaults(builder, tls, provider, &outbound_tls::outbound_global_default())
}

pub(crate) fn apply_resolved_outbound_tls_with_defaults(
    builder: reqwest::ClientBuilder,
    tls: &TlsConfig,
    provider: &str,
    defaults: &outbound_tls::GlobalOutboundTlsSnapshot,
) -> Result<reqwest::ClientBuilder> {
    let resolved = outbound_tls::resolve_outbound_tls_audited_with_defaults(
        &tls.outbound,
        tls.resolved_ca_certificates.as_deref(),
        tls.resolved_client_certificate.as_ref(),
        true,
        provider,
        defaults,
    )
    .map_err(|e| anyhow::anyhow!("{provider} TLS configuration error: {e:?}"))?;

    apply_effective_outbound_tls(builder, &resolved, provider)
}

pub(crate) fn apply_validated_outbound_tls(
    builder: reqwest::ClientBuilder,
    proof: &outbound_tls::ValidatedLocalTlsSelection<'_>,
    provider: &str,
    defaults: &outbound_tls::GlobalOutboundTlsSnapshot,
) -> Result<reqwest::ClientBuilder> {
    let resolved = outbound_tls::resolve_validated_outbound_tls_audited(proof, true, provider, defaults)
        .map_err(|e| anyhow::anyhow!("{provider} TLS configuration error: {e:?}"))?;
    apply_effective_outbound_tls(builder, &resolved, provider)
}

fn apply_effective_outbound_tls(
    builder: reqwest::ClientBuilder,
    resolved: &outbound_tls::ResolvedOutboundTls,
    provider: &str,
) -> Result<reqwest::ClientBuilder> {
    resolved.reject_unsupported_identity_policy(provider)?;

    if !resolved.verify {
        // Canonical resolution already warned and omitted client identity (M4).
        return Ok(builder.tls_backend_rustls().tls_danger_accept_invalid_certs(true));
    }
    let config =
        build_http_rustls_client_config(resolved).with_context(|| format!("{provider} TLS: configuration error"))?;
    Ok(builder.tls_backend_preconfigured(config))
}

#[cfg(test)]
#[path = "../../tests/unit/link_sys_core/tls_test.rs"]
mod tests;
