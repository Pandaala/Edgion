use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use time::OffsetDateTime;
use zeroize::Zeroizing;

/// A signed leaf cert and its private key in PEM format. The key is zeroized on drop.
pub struct LeafPem {
    pub cert_pem: String,
    pub key_pem: Zeroizing<String>,
}

/// Internal CA: holds the root cert PEM, the root key (zeroized on drop), and a
/// rcgen `Issuer` that can sign leaves without re-parsing the CA cert each time.
///
/// Created fresh via [`Ca::generate_root`] or reconstructed from a persisted
/// Kubernetes Secret via [`Ca::load_from_pem`].
///
/// Does NOT derive `JsonSchema` — this struct holds a private key.
pub struct Ca {
    ca_cert_pem: String,
    ca_key_pem: Zeroizing<String>,
    issuer: Issuer<'static, KeyPair>,
}

/// Root CA validity params. Roots are pre-distributed (baked into the leaf's
/// trust anchor at provisioning time), not raced the way a leaf is against a
/// concurrently-connecting gateway, so `not_before` is left at `now_utc()` —
/// no backdate slack needed here.
fn ca_params(ttl: time::Duration) -> Result<CertificateParams> {
    let mut p = CertificateParams::default();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "edgion-grpc-ca");
        dn
    };
    p.not_before = OffsetDateTime::now_utc();
    p.not_after = p
        .not_before
        .checked_add(ttl)
        .context("CA validity exceeds supported date range")?;
    Ok(p)
}

impl Ca {
    /// Generate a fresh self-signed ECDSA P-256 root CA valid for `ttl`.
    ///
    /// Internally serialises the cert and key to PEM then delegates to
    /// [`Ca::load_from_pem`] so there is a single owned-`Issuer` construction path.
    pub fn generate_root(ttl: time::Duration) -> Result<Ca> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("CA keygen")?;
        let params = ca_params(ttl)?;
        let cert = params.self_signed(&key).context("self-sign CA cert")?;
        let cert_pem = cert.pem();
        let key_pem = key.serialize_pem();
        Ca::load_from_pem(&cert_pem, &key_pem)
    }

    /// Re-self-sign the root with the SAME private key and a fresh validity window
    /// (spec §4.5). Public key unchanged => peers holding the old root cert continue
    /// to validate chains signed after renewal; no coordination needed.
    pub fn renew_root_same_key(&self, ca_ttl: time::Duration) -> Result<Ca> {
        let key = KeyPair::from_pem(self.ca_key_pem()).context("reload CA key for renewal")?;
        let params = ca_params(ca_ttl)?;
        let cert = params.self_signed(&key).context("re-self-sign CA cert")?;
        Ca::load_from_pem(&cert.pem(), self.ca_key_pem())
    }

    /// Reconstruct a `Ca` from persisted PEM strings.
    ///
    /// Uses rcgen 0.14's `Issuer::from_ca_cert_pem` (gated on the `x509-parser`
    /// feature) which builds an owned `Issuer` — no borrowed `CertificateParams`
    /// reference, so the lifetime resolves to `'static`.
    pub fn load_from_pem(ca_cert_pem: &str, ca_key_pem: &str) -> Result<Ca> {
        let key = KeyPair::from_pem(ca_key_pem).context("parse CA key PEM")?;
        let issuer: Issuer<'static, KeyPair> =
            Issuer::from_ca_cert_pem(ca_cert_pem, key).context("rebuild CA issuer from PEM")?;
        Ok(Ca {
            ca_cert_pem: ca_cert_pem.to_string(),
            ca_key_pem: Zeroizing::new(ca_key_pem.to_string()),
            issuer,
        })
    }

    /// Return the CA certificate PEM (public, safe to log / store).
    pub fn ca_cert_pem(&self) -> &str {
        &self.ca_cert_pem
    }

    /// Return the CA private-key PEM (secret; zeroized on drop).
    pub fn ca_key_pem(&self) -> &Zeroizing<String> {
        &self.ca_key_pem
    }

    /// Sign a server leaf cert with the supplied SANs (DNS names or IPs accepted
    /// by `CertificateParams::new`).  CN=edgion-controller, EKU=serverAuth.
    ///
    /// `not_before` is backdated by `BACKDATE_SLACK` to absorb clock skew between
    /// controller and gateway (spec §8 F7): a gateway whose clock lags slightly
    /// behind the controller must not see the leaf as "not yet valid" the instant
    /// it is minted. `not_after` counts `ttl` forward from `now`, NOT from the
    /// backdated `not_before` -- the requested TTL is the cert's actual lifetime
    /// budget, not TTL-plus-slack.
    pub fn sign_server(&self, sans: &[String], ttl: time::Duration) -> Result<LeafPem> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("server keygen")?;
        let mut p = CertificateParams::new(sans.to_vec()).context("server cert params")?;
        p.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "edgion-controller");
            dn
        };
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        p.not_before = now - super::cert_util::BACKDATE_SLACK;
        p.not_after = now
            .checked_add(ttl)
            .context("leaf validity exceeds supported date range")?;
        let cert = p.signed_by(&key, &self.issuer).context("sign server cert")?;
        Ok(LeafPem {
            cert_pem: cert.pem(),
            key_pem: Zeroizing::new(key.serialize_pem()),
        })
    }

    /// Sign a client leaf cert.  CN=edgion-gateway, EKU=clientAuth.
    ///
    /// Same backdate/TTL rules as [`Ca::sign_server`] -- see its doc comment.
    pub fn sign_client(&self, ttl: time::Duration) -> Result<LeafPem> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("client keygen")?;
        let mut p = CertificateParams::default();
        p.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "edgion-gateway");
            dn
        };
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let now = OffsetDateTime::now_utc();
        p.not_before = now - super::cert_util::BACKDATE_SLACK;
        p.not_after = now
            .checked_add(ttl)
            .context("leaf validity exceeds supported date range")?;
        let cert = p.signed_by(&key, &self.issuer).context("sign client cert")?;
        Ok(LeafPem {
            cert_pem: cert.pem(),
            key_pem: Zeroizing::new(key.serialize_pem()),
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/services/conf_sync_ca/ca_test.rs"]
mod tests;
