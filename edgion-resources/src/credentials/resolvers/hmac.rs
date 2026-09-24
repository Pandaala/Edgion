use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::HmacMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Webhook HMAC auth.
///
/// Secret key (camelCase, spec §5):
/// - `hmacKey` — required, the symmetric key used for payload signing.
#[derive(Debug, Default, Clone, Copy)]
pub struct HmacResolver;

impl SecretMaterialResolver for HmacResolver {
    type Material = HmacMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["hmacKey"]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let raw = data
            .get("hmacKey")
            .ok_or_else(|| ResolveError::MissingKeys(vec!["hmacKey"]))?;
        let key = std::str::from_utf8(&raw.0)
            .map_err(|e| ResolveError::DecodeError {
                key: "hmacKey",
                source: anyhow::anyhow!(e),
            })?
            .to_string();
        Ok(HmacMaterial { key })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/credentials/resolvers/hmac_test.rs"]
mod tests;
