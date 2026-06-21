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

fn ca_params(ca_days: u32) -> CertificateParams {
    let mut p = CertificateParams::default();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "edgion-grpc-ca");
        dn
    };
    p.not_before = OffsetDateTime::now_utc();
    p.not_after = OffsetDateTime::now_utc() + time::Duration::days(ca_days as i64);
    p
}

impl Ca {
    /// Generate a fresh self-signed ECDSA P-256 root CA valid for `ca_days` days.
    ///
    /// Internally serialises the cert and key to PEM then delegates to
    /// [`Ca::load_from_pem`] so there is a single owned-`Issuer` construction path.
    pub fn generate_root(ca_days: u32) -> Result<Ca> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("CA keygen")?;
        let params = ca_params(ca_days);
        let cert = params.self_signed(&key).context("self-sign CA cert")?;
        let cert_pem = cert.pem();
        let key_pem = key.serialize_pem();
        Ca::load_from_pem(&cert_pem, &key_pem)
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
    pub fn sign_server(&self, sans: &[String], cert_days: u32) -> Result<LeafPem> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("server keygen")?;
        let mut p = CertificateParams::new(sans.to_vec()).context("server cert params")?;
        p.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "edgion-controller");
            dn
        };
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.not_before = OffsetDateTime::now_utc();
        p.not_after = OffsetDateTime::now_utc() + time::Duration::days(cert_days as i64);
        let cert = p.signed_by(&key, &self.issuer).context("sign server cert")?;
        Ok(LeafPem {
            cert_pem: cert.pem(),
            key_pem: Zeroizing::new(key.serialize_pem()),
        })
    }

    /// Sign a client leaf cert.  CN=edgion-gateway, EKU=clientAuth.
    pub fn sign_client(&self, cert_days: u32) -> Result<LeafPem> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).context("client keygen")?;
        let mut p = CertificateParams::default();
        p.distinguished_name = {
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "edgion-gateway");
            dn
        };
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        p.not_before = OffsetDateTime::now_utc();
        p.not_after = OffsetDateTime::now_utc() + time::Duration::days(cert_days as i64);
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
        let ca = Ca::generate_root(3650).unwrap();

        // -- server leaf --
        let san_name = "edgion-controller.ns1.svc".to_string();
        let server = ca.sign_server(&[san_name.clone()], 365).unwrap();
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
        let client = ca.sign_client(365).unwrap();
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
        let ca = Ca::generate_root(3650).unwrap();
        let reloaded = Ca::load_from_pem(ca.ca_cert_pem(), ca.ca_key_pem()).unwrap();
        // A leaf signed by the reloaded CA must chain to the original root.
        let leaf = reloaded.sign_client(30).unwrap();
        // Chain check: the leaf's issuer DN must equal the root's subject DN.
        let root = parse(ca.ca_cert_pem());
        let l = parse(&leaf.cert_pem);
        assert_eq!(l.issuer().to_string(), root.subject().to_string());
    }
}
