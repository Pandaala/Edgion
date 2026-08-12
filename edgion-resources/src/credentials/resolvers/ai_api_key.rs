//! Resolver for the fixed `EdgionBackend.spec.ai` provider API-key Secret shape.

use std::collections::BTreeMap;

use k8s_openapi::ByteString;

use crate::constants::secret_keys::ai::API_KEY;

use super::super::{
    materials::AiApiKeyMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

#[derive(Debug, Default, Clone, Copy)]
pub struct AiApiKeyResolver;

impl SecretMaterialResolver for AiApiKeyResolver {
    type Material = AiApiKeyMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &[API_KEY]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let raw = data
            .get(API_KEY)
            .ok_or_else(|| ResolveError::MissingKeys(vec![API_KEY]))?;
        let api_key = std::str::from_utf8(&raw.0)
            .map_err(|source| ResolveError::DecodeError {
                key: API_KEY,
                source: anyhow::anyhow!(source),
            })?
            .to_string();
        Ok(AiApiKeyMaterial { api_key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixed_api_key() {
        let data = BTreeMap::from([(API_KEY.to_string(), ByteString(b"provider-key".to_vec()))]);
        let material = AiApiKeyResolver.parse(&data).unwrap();
        assert_eq!(material.api_key, "provider-key");
    }

    #[test]
    fn rejects_missing_or_non_utf8_api_key() {
        assert!(matches!(
            AiApiKeyResolver.parse(&BTreeMap::new()),
            Err(ResolveError::MissingKeys(keys)) if keys == vec![API_KEY]
        ));
        let data = BTreeMap::from([(API_KEY.to_string(), ByteString(vec![0xff]))]);
        assert!(matches!(
            AiApiKeyResolver.parse(&data),
            Err(ResolveError::DecodeError { key: API_KEY, .. })
        ));
    }
}
