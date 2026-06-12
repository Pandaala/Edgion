//! Certificate generation for conf_sync gRPC mTLS.
//!
//! Uses rcgen to generate a self-signed CA, then signs server and client certificates.

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use time::OffsetDateTime;

/// All generated certificates and keys in PEM format.
pub struct CertBundle {
    pub ca_cert_pem: String,
    // TODO: ca_key_pem not stored — Center federation may need it later
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

/// Options for certificate generation.
pub struct GenCertsOptions {
    /// CA certificate validity in days (default: 3650)
    pub ca_days: u32,
    /// Server/client certificate validity in days (default: 365)
    pub cert_days: u32,
    /// Additional server SANs (DNS names)
    pub server_sans: Vec<String>,
    /// K8s namespace (used to build service FQDN SAN)
    pub namespace: Option<String>,
}

/// Generate a complete certificate bundle for conf_sync mTLS.
///
/// Creates:
/// 1. Self-signed CA (ECDSA P-256)
/// 2. Server certificate signed by CA (for Controller)
/// 3. Client certificate signed by CA (for Gateway)
pub fn generate_certs(opts: &GenCertsOptions) -> Result<CertBundle> {
    // ── 1. Generate CA ──────────────────────────────────────────────
    let ca_key_pair =
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("Failed to generate CA key pair")?;

    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "edgion-grpc-ca");
        dn
    };
    ca_params.not_before = OffsetDateTime::now_utc();
    ca_params.not_after = OffsetDateTime::now_utc() + time::Duration::days(opts.ca_days as i64);

    // Build an Issuer (borrows ca_params + ca_key_pair) before consuming ca_params.
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key_pair);

    let ca_cert = ca_params
        .self_signed(&ca_key_pair)
        .context("Failed to self-sign CA certificate")?;

    let ca_cert_pem = ca_cert.pem();

    // ── 2. Generate Server Certificate ──────────────────────────────
    let server_key_pair =
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("Failed to generate server key pair")?;

    // Build SANs: DNS names + IP addresses (loopback + user-provided)
    // CertificateParams::new() auto-detects IP vs DNS from string format
    let mut server_sans = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(), // IPv4 loopback — needed when Gateway connects by IP
        "::1".to_string(),       // IPv6 loopback
    ];
    if let Some(ref ns) = opts.namespace {
        server_sans.push(format!("edgion-controller.{}.svc.cluster.local", ns));
        server_sans.push(format!("edgion-controller.{}.svc", ns));
        server_sans.push("edgion-controller".to_string());
    }
    server_sans.extend(opts.server_sans.clone());

    let mut server_params = CertificateParams::new(server_sans).context("Failed to create server cert params")?;
    server_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "edgion-controller");
        dn
    };
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.not_before = OffsetDateTime::now_utc();
    server_params.not_after = OffsetDateTime::now_utc() + time::Duration::days(opts.cert_days as i64);

    let server_cert = server_params
        .signed_by(&server_key_pair, &ca_issuer)
        .context("Failed to sign server certificate")?;

    // ── 3. Generate Client Certificate ──────────────────────────────
    let client_key_pair =
        KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("Failed to generate client key pair")?;

    let mut client_params = CertificateParams::default();
    client_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "edgion-gateway");
        dn
    };
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    client_params.not_before = OffsetDateTime::now_utc();
    client_params.not_after = OffsetDateTime::now_utc() + time::Duration::days(opts.cert_days as i64);

    let client_cert = client_params
        .signed_by(&client_key_pair, &ca_issuer)
        .context("Failed to sign client certificate")?;

    Ok(CertBundle {
        ca_cert_pem,
        server_cert_pem: server_cert.pem(),
        server_key_pem: server_key_pair.serialize_pem(),
        client_cert_pem: client_cert.pem(),
        client_key_pem: client_key_pair.serialize_pem(),
    })
}
