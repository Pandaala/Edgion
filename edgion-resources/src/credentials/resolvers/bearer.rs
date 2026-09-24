use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::BearerMaterial,
    resolver::{InvalidValueClass, ResolveError, SecretMaterialResolver},
};

pub const BEARER_TOKEN_KEY: &str = "bearerToken";

/// Bearer token auth — shared by Elasticsearch bearer and Webhook bearer.
///
/// Secret key (camelCase, spec §5):
/// - `bearerToken` — required
#[derive(Debug, Default, Clone, Copy)]
pub struct BearerResolver;

impl SecretMaterialResolver for BearerResolver {
    type Material = BearerMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &[BEARER_TOKEN_KEY]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let raw = data
            .get(BEARER_TOKEN_KEY)
            .ok_or_else(|| ResolveError::MissingKeys(vec![BEARER_TOKEN_KEY]))?;
        let token = std::str::from_utf8(&raw.0).map_err(|e| ResolveError::DecodeError {
            key: BEARER_TOKEN_KEY,
            source: anyhow::anyhow!(e),
        })?;
        let padding_start = token.find('=').unwrap_or(token.len());
        let (value, padding) = token.split_at(padding_start);
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/'))
            || !padding.bytes().all(|byte| byte == b'=')
        {
            return Err(ResolveError::InvalidValue {
                key: BEARER_TOKEN_KEY,
                class: InvalidValueClass::BearerToken,
            });
        }
        let token = token.to_string();
        Ok(BearerMaterial { token })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/credentials/resolvers/bearer_test.rs"]
mod tests;
