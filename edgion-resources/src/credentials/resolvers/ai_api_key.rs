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
#[path = "../../../tests/unit/credentials/resolvers/ai_api_key_test.rs"]
mod tests;
