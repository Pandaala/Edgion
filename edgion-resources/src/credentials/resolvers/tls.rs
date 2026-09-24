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
#[path = "../../../tests/unit/credentials/resolvers/tls_test.rs"]
mod tests;
