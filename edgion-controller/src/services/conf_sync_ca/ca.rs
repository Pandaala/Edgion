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
fn ca_params(ttl: time::Duration) -> CertificateParams {
    let mut p = CertificateParams::default();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "edgion-grpc-ca");
        dn
    };
    p.not_before = OffsetDateTime::now_utc();
    p.not_after = OffsetDateTime::now_utc() + ttl;
    p
}

impl Ca {
    /// Generate a fresh self-signed ECDSA P-256 root CA valid for `ttl`.
    ///
    /// Internally serialises the cert and key to PEM then delegates to
    /// [`Ca::load_from_pem`] so there is a single owned-`Issuer` construction path.
    pub fn generate_root(ttl: time::Duration) -> Result<Ca> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("CA keygen")?;
        let params = ca_params(ttl);
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
        let params = ca_params(ca_ttl);
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
        p.not_before = OffsetDateTime::now_utc() - super::cert_util::BACKDATE_SLACK;
        p.not_after = OffsetDateTime::now_utc() + ttl;
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
        p.not_before = OffsetDateTime::now_utc() - super::cert_util::BACKDATE_SLACK;
        p.not_after = OffsetDateTime::now_utc() + ttl;
        let cert = p.signed_by(&key, &self.issuer).context("sign client cert")?;
        Ok(LeafPem {
            cert_pem: cert.pem(),
            key_pem: Zeroizing::new(key.serialize_pem()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x509_parser::prelude::*;

    /// Parse a PEM certificate, leaking the DER buffer so the returned
    /// `X509Certificate` can carry a `'static` lifetime (test-only helper).
    fn parse(pem: &str) -> X509Certificate<'static> {
        let der = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap().1;
        let boxed: &'static [u8] = Box::leak(der.contents.into_boxed_slice());
        X509Certificate::from_der(boxed).unwrap().1
    }

    #[test]
    fn root_and_leaves_have_expected_shape() {
        let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();

        // -- server leaf --
        let san_name = "edgion-controller.ns1.svc".to_string();
        let server = ca
            .sign_server(&[san_name.clone()], ::time::Duration::days(365))
            .unwrap();
        let sc = parse(&server.cert_pem);

        // Real EKU assertion: server cert must carry serverAuth.
        let server_eku = sc
            .extended_key_usage()
            .expect("server EKU parse ok")
            .expect("server cert must have an EKU extension");
        assert!(server_eku.value.server_auth, "server cert EKU must include serverAuth");

        // SAN assertion: the requested DNS name must appear in the SAN extension.
        let san_ext = sc
            .subject_alternative_name()
            .expect("server SAN parse ok")
            .expect("server cert must have a SAN extension");
        let has_san = san_ext
            .value
            .general_names
            .iter()
            .any(|n| matches!(n, GeneralName::DNSName(name) if *name == san_name.as_str()));
        assert!(has_san, "server cert SAN must contain '{san_name}'");

        // -- client leaf --
        let client = ca.sign_client(::time::Duration::days(365)).unwrap();
        let cc = parse(&client.cert_pem);

        // Real EKU assertion: client cert must carry clientAuth.
        let client_eku = cc
            .extended_key_usage()
            .expect("client EKU parse ok")
            .expect("client cert must have an EKU extension");
        assert!(client_eku.value.client_auth, "client cert EKU must include clientAuth");

        // CN assertion.
        assert!(cc.subject().to_string().contains("edgion-gateway"));

        // Both leaf private keys must be non-empty PEM.
        assert!(server.key_pem.starts_with("-----BEGIN"));
        assert!(client.key_pem.starts_with("-----BEGIN"));
    }

    #[test]
    fn load_from_pem_reissues_against_same_root() {
        let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
        let reloaded = Ca::load_from_pem(ca.ca_cert_pem(), ca.ca_key_pem()).unwrap();
        // A leaf signed by the reloaded CA must chain to the original root.
        let leaf = reloaded.sign_client(::time::Duration::days(30)).unwrap();
        // Chain check: the leaf's issuer DN must equal the root's subject DN.
        let root = parse(ca.ca_cert_pem());
        let l = parse(&leaf.cert_pem);
        assert_eq!(l.issuer().to_string(), root.subject().to_string());
    }

    #[test]
    fn leaves_are_backdated_five_minutes() {
        use crate::services::conf_sync_ca::cert_util::{parse_cert_window, BACKDATE_SLACK};
        let ca = Ca::generate_root(::time::Duration::days(365)).unwrap();
        let leaf = ca.sign_client(::time::Duration::days(30)).unwrap();
        let (nb, _) = parse_cert_window(&leaf.cert_pem).unwrap();
        let now = ::time::OffsetDateTime::now_utc();
        assert!(nb <= now - BACKDATE_SLACK + ::time::Duration::seconds(30));
        assert!(nb >= now - BACKDATE_SLACK - ::time::Duration::seconds(30));
    }

    #[test]
    fn renew_root_same_key_keeps_trust() {
        let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
        let renewed = ca.renew_root_same_key(::time::Duration::days(3650)).unwrap();
        // Same public key: a leaf signed by the RENEWED root verifies against the OLD root cert's key.
        let leaf = renewed.sign_client(::time::Duration::days(30)).unwrap();
        assert!(edgion_common::grpc_tls::validate_identity(
            leaf.cert_pem.as_bytes(),
            leaf.key_pem.as_bytes(),
            ca.ca_cert_pem().as_bytes(), // OLD root cert as trust anchor
        )
        .is_ok());
        // And the window actually moved forward.
        use crate::services::conf_sync_ca::cert_util::parse_cert_window;
        let (_, old_na) = parse_cert_window(ca.ca_cert_pem()).unwrap();
        let (_, new_na) = parse_cert_window(renewed.ca_cert_pem()).unwrap();
        assert!(new_na >= old_na);
    }
}
