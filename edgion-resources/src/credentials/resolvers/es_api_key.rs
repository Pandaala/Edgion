use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::EsApiKeyMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Elasticsearch apiKey auth.
///
/// Secret keys (camelCase, spec §5):
/// - `apiKeyId` — required
/// - `apiKeySecret` — required
///
/// All missing keys are reported in a single error (multi-key contract).
#[derive(Debug, Default, Clone, Copy)]
pub struct EsApiKeyResolver;

impl SecretMaterialResolver for EsApiKeyResolver {
    type Material = EsApiKeyMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["apiKeyId", "apiKeySecret"]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let mut missing = Vec::new();
        if !data.contains_key("apiKeyId") {
            missing.push("apiKeyId");
        }
        if !data.contains_key("apiKeySecret") {
            missing.push("apiKeySecret");
        }
        if !missing.is_empty() {
            return Err(ResolveError::MissingKeys(missing));
        }

        let api_key_id = decode_utf8(data, "apiKeyId")?;
        let api_key_secret = decode_utf8(data, "apiKeySecret")?;
        Ok(EsApiKeyMaterial {
            api_key_id,
            api_key_secret,
        })
    }
}

fn decode_utf8(data: &BTreeMap<String, ByteString>, key: &'static str) -> Result<String, ResolveError> {
    let raw = data.get(key).expect("presence already checked");
    std::str::from_utf8(&raw.0)
        .map(|s| s.to_string())
        .map_err(|e| ResolveError::DecodeError {
            key,
            source: anyhow::anyhow!(e),
        })
}

#[cfg(test)]
#[path = "../../../tests/unit/credentials/resolvers/es_api_key_test.rs"]
mod tests;
