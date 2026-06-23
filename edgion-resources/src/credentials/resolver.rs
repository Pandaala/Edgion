//! [`SecretMaterialResolver`] trait and error types.
//!
//! A resolver maps raw `Secret.data` bytes into a strongly-typed
//! [`Material`](SecretMaterialResolver::Material). It does **not** perform
//! namespace checks, SecretRefManager registration, or status management —
//! those concerns live in the Controller-side pipeline.

use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use thiserror::Error;

/// Resolver responsibility: declare required keys + translate `Secret.data`
/// into the corresponding [`Material`](Self::Material).
pub trait SecretMaterialResolver: Send + Sync {
    type Material: Send + Sync;

    /// Keys that MUST exist in `Secret.data` (camelCase, see LinkSys spec §5).
    fn required_keys(&self) -> &'static [&'static str];

    /// Whitelist of `Secret.type` values that this resolver accepts.
    ///
    /// Semantics:
    /// - The sentinel value `""` in the returned slice means the `type` field
    ///   may also be absent / unset on the `Secret` object.
    /// - An unknown type yields [`ResolveError::InvalidSecretType`] from the
    ///   Controller-side pipeline (resolvers themselves only care about data).
    ///
    /// Default: accept `"Opaque"` and missing type. Override for stricter
    /// validation (e.g. `kubernetes.io/basic-auth`, `kubernetes.io/tls`).
    fn allowed_secret_types(&self) -> &'static [&'static str] {
        &["Opaque", ""]
    }

    /// Parse `Secret.data` into a [`Material`](Self::Material).
    ///
    /// Missing keys yield [`ResolveError::MissingKeys`] (all missing keys
    /// reported at once). Decode failures yield [`ResolveError::DecodeError`].
    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError>;
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("missing keys in Secret: {0:?}")]
    MissingKeys(Vec<&'static str>),

    #[error("failed to decode key `{key}`: {source}")]
    DecodeError {
        key: &'static str,
        #[source]
        source: anyhow::Error,
    },

    #[error("Secret type {got:?} not in allowed set {allowed:?}")]
    InvalidSecretType {
        got: String,
        allowed: &'static [&'static str],
    },
}

#[cfg(test)]
mod tests {
    use super::{ResolveError, SecretMaterialResolver};
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    struct DummyResolver;
    #[derive(Debug)]
    struct DummyMaterial(String);

    impl SecretMaterialResolver for DummyResolver {
        type Material = DummyMaterial;
        fn required_keys(&self) -> &'static [&'static str] {
            &["alpha", "beta"]
        }
        fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
            let alpha = data
                .get("alpha")
                .ok_or_else(|| ResolveError::MissingKeys(vec!["alpha"]))?;
            let value = std::str::from_utf8(&alpha.0).map_err(|e| ResolveError::DecodeError {
                key: "alpha",
                source: anyhow::anyhow!(e),
            })?;
            Ok(DummyMaterial(value.to_string()))
        }
    }

    #[test]
    fn required_keys_exposed() {
        assert_eq!(DummyResolver.required_keys(), &["alpha", "beta"]);
    }

    #[test]
    fn default_allowed_types_is_opaque() {
        assert_eq!(DummyResolver.allowed_secret_types(), &["Opaque", ""]);
    }

    #[test]
    fn parse_happy_path() {
        let mut data = BTreeMap::new();
        data.insert("alpha".to_string(), ByteString(b"hello".to_vec()));
        data.insert("beta".to_string(), ByteString(b"world".to_vec()));
        let m = DummyResolver.parse(&data).unwrap();
        assert_eq!(m.0, "hello");
    }

    #[test]
    fn parse_missing_key() {
        let data = BTreeMap::new();
        match DummyResolver.parse(&data) {
            Err(ResolveError::MissingKeys(keys)) => assert_eq!(keys, vec!["alpha"]),
            other => panic!("expected MissingKeys, got {other:?}"),
        }
    }

    #[test]
    fn parse_decode_error() {
        let mut data = BTreeMap::new();
        data.insert("alpha".to_string(), ByteString(vec![0xff, 0xfe]));
        match DummyResolver.parse(&data) {
            Err(ResolveError::DecodeError { key, .. }) => assert_eq!(key, "alpha"),
            other => panic!("expected DecodeError, got {other:?}"),
        }
    }
}
