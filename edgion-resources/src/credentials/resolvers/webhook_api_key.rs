use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::WebhookApiKeyMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Webhook API Key auth.
///
/// Secret key (camelCase, spec §5):
/// - `apiKey` — required
#[derive(Debug, Default, Clone, Copy)]
pub struct WebhookApiKeyResolver;

impl SecretMaterialResolver for WebhookApiKeyResolver {
    type Material = WebhookApiKeyMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["apiKey"]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let raw = data
            .get("apiKey")
            .ok_or_else(|| ResolveError::MissingKeys(vec!["apiKey"]))?;
        let api_key = std::str::from_utf8(&raw.0)
            .map_err(|e| ResolveError::DecodeError {
                key: "apiKey",
                source: anyhow::anyhow!(e),
            })?
            .to_string();
        Ok(WebhookApiKeyMaterial { api_key })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/credentials/resolvers/webhook_api_key_test.rs"]
mod tests;
