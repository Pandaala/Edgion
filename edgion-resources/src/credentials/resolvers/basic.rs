use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::BasicAuthMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Shared resolver for Redis / Etcd / ES basic / Webhook basic auth.
///
/// Secret keys (camelCase, spec §5):
/// - `username` — optional; when absent, the client connects with the default ACL
/// - `password` — required
///
/// Accepted Secret types: `kubernetes.io/basic-auth` and `Opaque` (missing
/// type is also allowed and treated as `Opaque`).
#[derive(Debug, Default, Clone, Copy)]
pub struct BasicAuthResolver;

impl SecretMaterialResolver for BasicAuthResolver {
    type Material = BasicAuthMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["password"]
    }

    fn allowed_secret_types(&self) -> &'static [&'static str] {
        &["kubernetes.io/basic-auth", "Opaque", ""]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let password_raw = data
            .get("password")
            .ok_or_else(|| ResolveError::MissingKeys(vec!["password"]))?;
        let password = std::str::from_utf8(&password_raw.0)
            .map_err(|e| ResolveError::DecodeError {
                key: "password",
                source: anyhow::anyhow!(e),
            })?
            .to_string();

        let username = match data.get("username") {
            Some(raw) => Some(
                std::str::from_utf8(&raw.0)
                    .map_err(|e| ResolveError::DecodeError {
                        key: "username",
                        source: anyhow::anyhow!(e),
                    })?
                    .to_string(),
            ),
            None => None,
        };

        Ok(BasicAuthMaterial { username, password })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/credentials/resolvers/basic_test.rs"]
mod tests;
