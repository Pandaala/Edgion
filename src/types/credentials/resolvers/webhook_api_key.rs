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
mod tests {
    use super::*;
    use crate::types::credentials::{ResolveError, SecretMaterialResolver};

    #[test]
    fn parse_happy_path() {
        let mut data = BTreeMap::new();
        data.insert("apiKey".to_string(), ByteString(b"my-secret-key".to_vec()));
        let m = WebhookApiKeyResolver.parse(&data).unwrap();
        assert_eq!(m.api_key, "my-secret-key");
    }

    #[test]
    fn parse_missing() {
        let data = BTreeMap::new();
        match WebhookApiKeyResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["apiKey"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_invalid_utf8() {
        let mut data = BTreeMap::new();
        data.insert("apiKey".to_string(), ByteString(vec![0xff, 0xfe]));
        match WebhookApiKeyResolver.parse(&data) {
            Err(ResolveError::DecodeError { key: "apiKey", .. }) => {}
            other => panic!("{other:?}"),
        }
    }
}
