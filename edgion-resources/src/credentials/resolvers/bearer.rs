use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::BearerMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Bearer token auth — shared by Elasticsearch bearer and Webhook bearer.
///
/// Secret key (camelCase, spec §5):
/// - `bearerToken` — required
#[derive(Debug, Default, Clone, Copy)]
pub struct BearerResolver;

impl SecretMaterialResolver for BearerResolver {
    type Material = BearerMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["bearerToken"]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let raw = data
            .get("bearerToken")
            .ok_or_else(|| ResolveError::MissingKeys(vec!["bearerToken"]))?;
        let token = std::str::from_utf8(&raw.0)
            .map_err(|e| ResolveError::DecodeError {
                key: "bearerToken",
                source: anyhow::anyhow!(e),
            })?
            .to_string();
        Ok(BearerMaterial { token })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{ResolveError, SecretMaterialResolver};

    #[test]
    fn parse_happy_path() {
        let mut data = BTreeMap::new();
        data.insert("bearerToken".to_string(), ByteString(b"tk".to_vec()));
        let m = BearerResolver.parse(&data).unwrap();
        assert_eq!(m.token, "tk");
    }

    #[test]
    fn parse_missing() {
        let data = BTreeMap::new();
        match BearerResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["bearerToken"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_invalid_utf8() {
        let mut data = BTreeMap::new();
        data.insert("bearerToken".to_string(), ByteString(vec![0xff, 0xfe]));
        match BearerResolver.parse(&data) {
            Err(ResolveError::DecodeError { key: "bearerToken", .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_extra_keys_ignored() {
        let mut data = BTreeMap::new();
        data.insert("bearerToken".to_string(), ByteString(b"tk".to_vec()));
        data.insert("unrelated".to_string(), ByteString(b"x".to_vec()));
        assert!(BearerResolver.parse(&data).is_ok());
    }
}
