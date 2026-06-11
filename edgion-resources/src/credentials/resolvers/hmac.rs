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
mod tests {
    use super::*;
    use crate::credentials::{ResolveError, SecretMaterialResolver};

    #[test]
    fn parse_happy_path() {
        let mut data = BTreeMap::new();
        data.insert("hmacKey".to_string(), ByteString(b"k".to_vec()));
        assert_eq!(HmacResolver.parse(&data).unwrap().key, "k");
    }

    #[test]
    fn parse_missing() {
        let data = BTreeMap::new();
        match HmacResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["hmacKey"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_invalid_utf8() {
        let mut data = BTreeMap::new();
        data.insert("hmacKey".to_string(), ByteString(vec![0xff, 0xfe]));
        match HmacResolver.parse(&data) {
            Err(ResolveError::DecodeError { key: "hmacKey", .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_extra_keys_ignored() {
        let mut data = BTreeMap::new();
        data.insert("hmacKey".to_string(), ByteString(b"k".to_vec()));
        data.insert("unrelated".to_string(), ByteString(b"x".to_vec()));
        assert!(HmacResolver.parse(&data).is_ok());
    }
}
