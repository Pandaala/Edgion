//! [`SecretMaterialResolver`] trait and error types.
//!
//! A resolver maps raw `Secret.data` bytes into a strongly-typed
//! [`Material`](SecretMaterialResolver::Material). It does **not** perform
//! namespace checks, SecretRefManager registration, or status management —
//! those concerns live in the Controller-side pipeline.

use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use thiserror::Error;

/// Fixed, non-user-controlled classification for a syntactically invalid
/// credential value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidValueClass {
    BearerToken,
}

impl std::fmt::Display for InvalidValueClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BearerToken => formatter.write_str("invalid bearer token"),
        }
    }
}

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
    /// Resolvers with a constrained value syntax use
    /// [`ResolveError::InvalidValue`] with a fixed classification.
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

    #[error("invalid value for key `{key}`: {class}")]
    InvalidValue {
        key: &'static str,
        class: InvalidValueClass,
    },

    #[error("Secret type {got:?} not in allowed set {allowed:?}")]
    InvalidSecretType {
        got: String,
        allowed: &'static [&'static str],
    },
}

#[cfg(test)]
#[path = "../../tests/unit/credentials/resolver_test.rs"]
mod tests;
