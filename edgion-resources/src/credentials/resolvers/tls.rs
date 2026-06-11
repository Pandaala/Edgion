//! TLS certificate resolver.
//!
//! Parses a `kubernetes.io/tls` (or `Opaque`) Secret into a [`TlsMaterial`].
//!
//! Supported keys (K8s conventions):
//! - `tls.crt` — client certificate (PEM) for mTLS
//! - `tls.key` — client **private** key (PEM); wrapped in `Zeroizing`
//! - `ca.crt` — CA certificate (PEM) used to verify the backend
//!
//! At least one of the three keys must be present; an otherwise empty Secret is
//! rejected with [`ResolveError::MissingKeys`] listing all three candidates, so
//! the caller can surface a helpful status message.

use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use zeroize::Zeroizing;

use super::super::{
    materials::TlsMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Standard key names looked up in `Secret.data`.
pub const TLS_CERT_KEY: &str = "tls.crt";
pub const TLS_KEY_KEY: &str = "tls.key";
pub const TLS_CA_KEY: &str = "ca.crt";

/// Candidate keys used for [`SecretMaterialResolver::required_keys`].
///
/// Kept separately so that the status message surfaces the full expected set
/// when none of them is present.
const TLS_CANDIDATE_KEYS: &[&str] = &[TLS_CERT_KEY, TLS_KEY_KEY, TLS_CA_KEY];

#[derive(Debug, Default, Clone, Copy)]
pub struct TlsCertsResolver;

impl SecretMaterialResolver for TlsCertsResolver {
    type Material = TlsMaterial;

    /// All three keys are advertised as "required" so that an empty Secret
    /// produces a comprehensive error message. The actual acceptance rule
    /// ("≥1 key present") is enforced in [`parse`].
    fn required_keys(&self) -> &'static [&'static str] {
        TLS_CANDIDATE_KEYS
    }

    fn allowed_secret_types(&self) -> &'static [&'static str] {
        &["kubernetes.io/tls", "Opaque", ""]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let ca_pem = data.get(TLS_CA_KEY).map(|v| v.0.clone());
        let client_cert_pem = data.get(TLS_CERT_KEY).map(|v| v.0.clone());
        let client_key_pem = data.get(TLS_KEY_KEY).map(|v| Zeroizing::new(v.0.clone()));

        if ca_pem.is_none() && client_cert_pem.is_none() && client_key_pem.is_none() {
            return Err(ResolveError::MissingKeys(TLS_CANDIDATE_KEYS.to_vec()));
        }

        Ok(TlsMaterial {
            ca_pem,
            client_cert_pem,
            client_key_pem,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(data: &[(&str, &[u8])]) -> BTreeMap<String, ByteString> {
        data.iter()
            .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
            .collect()
    }

    #[test]
    fn parse_full_kubernetes_tls_secret() {
        let data = mk(&[("tls.crt", b"CERT"), ("tls.key", b"KEY"), ("ca.crt", b"CA")]);
        let m = TlsCertsResolver.parse(&data).unwrap();
        assert_eq!(m.ca_pem.as_deref(), Some(b"CA".as_ref()));
        assert_eq!(m.client_cert_pem.as_deref(), Some(b"CERT".as_ref()));
        assert_eq!(m.client_key_pem.as_ref().map(|z| z.as_slice()), Some(b"KEY".as_ref()));
    }

    #[test]
    fn parse_ca_only() {
        let data = mk(&[("ca.crt", b"CA")]);
        let m = TlsCertsResolver.parse(&data).unwrap();
        assert!(m.ca_pem.is_some());
        assert!(m.client_cert_pem.is_none());
        assert!(m.client_key_pem.is_none());
    }

    #[test]
    fn parse_mtls_without_ca() {
        let data = mk(&[("tls.crt", b"CERT"), ("tls.key", b"KEY")]);
        let m = TlsCertsResolver.parse(&data).unwrap();
        assert!(m.ca_pem.is_none());
        assert!(m.client_cert_pem.is_some());
        assert!(m.client_key_pem.is_some());
    }

    #[test]
    fn parse_empty_reports_all_candidates() {
        let data = BTreeMap::new();
        match TlsCertsResolver.parse(&data) {
            Err(ResolveError::MissingKeys(keys)) => {
                assert_eq!(keys, vec!["tls.crt", "tls.key", "ca.crt"]);
            }
            other => panic!("expected MissingKeys, got {other:?}"),
        }
    }

    #[test]
    fn parse_preserves_binary_content_verbatim() {
        // TLS material is bytes; non-UTF8 is legal (DER, etc. — even though we
        // advertise PEM, the resolver should not try to validate).
        let data = mk(&[("tls.key", &[0x00, 0xff, 0x7f][..])]);
        let m = TlsCertsResolver.parse(&data).unwrap();
        assert_eq!(m.client_key_pem.as_ref().unwrap().as_slice(), &[0x00, 0xff, 0x7f]);
    }

    #[test]
    fn allows_k8s_tls_and_opaque_types() {
        let allowed = TlsCertsResolver.allowed_secret_types();
        assert!(allowed.contains(&"kubernetes.io/tls"));
        assert!(allowed.contains(&"Opaque"));
        assert!(allowed.contains(&""));
    }

    #[test]
    fn required_keys_match_constants() {
        assert_eq!(
            TlsCertsResolver.required_keys(),
            &[TLS_CERT_KEY, TLS_KEY_KEY, TLS_CA_KEY]
        );
    }
}
